//! Stream-based `FSEvents` interface.
#![allow(
    clippy::non_send_fields_in_send_ty,
    clippy::cast_possible_wrap,
    clippy::borrow_interior_mutable_const,
    clippy::module_name_repetitions
)]

use std::ffi::{c_void, CStr, OsStr};
use std::fmt::{Display, Formatter};
use std::io;
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt;
use std::panic::catch_unwind;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::mpsc::channel;
use std::task::{Context, Poll};
use std::thread;
use std::time::Duration;

#[cfg(feature = "async-std")]
use async_std1 as async_std;
use core_foundation::array::CFArray;
use core_foundation::base::{CFIndex, FromVoid};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::runloop::{kCFRunLoopBeforeWaiting, kCFRunLoopDefaultMode, CFRunLoop};
use core_foundation::string::CFString;
use either::Either;
use futures_core::Stream;
use futures_util::stream::{iter, StreamExt};
use log::{debug, error};
#[cfg(feature = "tokio")]
use tokio1 as tokio;
#[cfg(feature = "tokio")]
use tokio_stream::wrappers::ReceiverStream;

use crate::ffi::{
    kFSEventStreamCreateFlagFileEvents, kFSEventStreamCreateFlagUseCFTypes,
    kFSEventStreamCreateFlagUseExtendedData, kFSEventStreamEventExtendedDataPathKey,
    kFSEventStreamEventExtendedFileIDKey, CFRunLoopExt, FSEventStreamCreateFlags,
    FSEventStreamEventFlags, FSEventStreamEventId, SysFSEventStream, SysFSEventStreamContext,
    SysFSEventStreamRef,
};
pub use crate::flags::StreamFlags;
use crate::impl_release_callback;
use crate::observer::create_oneshot_observer;
use crate::utils::FlagsExt;

#[cfg(test)]
pub(crate) static TEST_RUNNING_RUNLOOP_COUNT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// An owned permission to stop an [`EventStream`](EventStream) and terminate its backing `RunLoop`.
///
/// A `EventStreamHandler` *detaches* the associated Stream and `RunLoop` when it is dropped, which
/// means that there is no longer any handle to them and no way to `abort` them.
///
/// Dropping the handler without first calling [`abort`](EventStreamHandler::abort) is not
/// recommended because this leaves a spawned thread behind and causes memory leaks.
pub struct EventStreamHandler {
    runloop: Option<(CFRunLoop, thread::JoinHandle<()>)>,
}

// Safety:
// - According to the Apple documentation, it's safe to move `CFRef`s across threads.
//   https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Multithreading/ThreadSafetySummary/ThreadSafetySummary.html
unsafe impl Send for EventStreamHandler {}

impl EventStreamHandler {
    /// Stop an [`EventStream`](EventStream) and terminate its backing `RunLoop`.
    ///
    /// Calling this method multiple times has no extra effect and won't cause any panic, error,
    /// or undefined behavior.
    pub fn abort(&mut self) {
        if let Some((runloop, thread_handle)) = self.runloop.take() {
            let (tx, rx) = channel();
            let observer = create_oneshot_observer(kCFRunLoopBeforeWaiting, tx);
            runloop.add_observer(&observer, unsafe { kCFRunLoopDefaultMode });

            if !runloop.is_waiting() {
                // Wait the RunLoop to enter Waiting state.
                rx.recv().expect("channel to receive BeforeWaiting signal");
            }

            runloop.remove_observer(&observer, unsafe { kCFRunLoopDefaultMode });
            runloop.stop();

            // Wait for the thread to shut down.
            thread_handle.join().expect("thread to shut down");
        }
    }
}

/// An `FSEvents` API event.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Event {
    pub path: PathBuf,
    pub inode: Option<i64>,
    pub flags: StreamFlags,
    pub raw_flags: FSEventStreamEventFlags,
    pub id: FSEventStreamEventId,
}

impl Display for Event {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] path: {:?}({}), flags: {} ({:x})",
            self.id,
            self.path,
            self.inode.unwrap_or(-1),
            self.flags,
            self.raw_flags
        )
    }
}

/// A stream of `FSEvents` API event batches.
///
/// You may want a stream of [`Event`](Event) instead of a stream of batches of it.
/// Call [`EventStream::into_flatten`](EventStream::into_flatten) to get one.
///
/// Call [`create_event_stream`](create_event_stream) to create it.
pub struct EventStream {
    #[cfg(feature = "tokio")]
    stream: ReceiverStream<Vec<Event>>,
    #[cfg(feature = "async-std")]
    stream: async_std::channel::Receiver<Vec<Event>>,
}

impl EventStream {
    /// Flatten event batches and produce a stream of [`Event`](Event).
    pub fn into_flatten(self) -> impl Stream<Item = Event> {
        self.flat_map(iter)
    }
}

impl Stream for EventStream {
    type Item = Vec<Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.poll_next_unpin(cx)
    }
}

pub(crate) struct StreamContextInfo {
    #[cfg(feature = "tokio")]
    event_handler: tokio::sync::mpsc::Sender<Vec<Event>>,
    #[cfg(feature = "async-std")]
    event_handler: async_std::channel::Sender<Vec<Event>>,
    create_flags: FSEventStreamCreateFlags,
}

impl_release_callback!(release_context, StreamContextInfo);

struct SendWrapper<T>(T);

unsafe impl<T> Send for SendWrapper<T> {}

impl<T> SendWrapper<T> {
    const unsafe fn new(t: T) -> Self {
        Self(t)
    }
}

/// Create a new [`EventStream`](EventStream) and [`EventStreamHandler`](EventStreamHandler) pair.
///
/// # Errors
/// Return error when there's any invalid path in `paths_to_watch`.
///
/// # Panics
/// Panic when the given flags combination is illegal.
pub fn create_event_stream<P: AsRef<Path>>(
    paths_to_watch: impl IntoIterator<Item = P>,
    since_when: FSEventStreamEventId,
    latency: Duration,
    flags: FSEventStreamCreateFlags,
) -> io::Result<(EventStream, EventStreamHandler)> {
    if flags.contains(kFSEventStreamCreateFlagUseExtendedData)
        && !flags.contains(kFSEventStreamCreateFlagUseCFTypes)
    {
        panic!("UseExtendedData requires UseCFTypes");
    }

    #[cfg(feature = "tokio")]
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(1024);
    #[cfg(feature = "async-std")]
    let (event_tx, event_rx) = async_std::channel::bounded(1024);

    // We need to associate the stream context with our callback in order to propagate events
    // to the rest of the system. This will be owned by the stream, and will be freed when the
    // stream is closed. This means we will leak the context if we panic before reacing
    // `FSEventStreamRelease`.
    let context = StreamContextInfo {
        event_handler: event_tx,
        create_flags: flags,
    };

    let stream_context = SysFSEventStreamContext::new(context, release_context);

    // We must append some additional flags because our callback parse them so
    let mut stream = SysFSEventStream::new(
        callback,
        &stream_context,
        paths_to_watch,
        since_when,
        latency,
        flags,
    )?;

    // channel to pass runloop around
    let (runloop_tx, runloop_rx) = channel();

    let thread_handle = thread::spawn(move || {
        #[cfg(test)]
        TEST_RUNNING_RUNLOOP_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let current_runloop = CFRunLoop::get_current();

        stream.schedule(&current_runloop, unsafe { kCFRunLoopDefaultMode });
        stream.start();

        // the calling to CFRunLoopRun will be terminated by CFRunLoopStop call in drop()
        // Safety:
        // - According to the Apple documentation, it's safe to move `CFRef`s across threads.
        //   https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Multithreading/ThreadSafetySummary/ThreadSafetySummary.html
        runloop_tx
            .send(unsafe { SendWrapper::new(current_runloop) })
            .expect("send runloop to stream");

        CFRunLoop::run_current();
        stream.stop();
        stream.invalidate();

        #[cfg(test)]
        TEST_RUNNING_RUNLOOP_COUNT.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    });

    #[cfg(feature = "tokio")]
    let stream = ReceiverStream::new(event_rx);
    #[cfg(feature = "async-std")]
    let stream = event_rx;
    Ok((
        EventStream { stream },
        EventStreamHandler {
            runloop: Some((
                runloop_rx.recv().expect("receive runloop from worker").0,
                thread_handle,
            )),
        },
    ))
}

extern "C" fn callback(
    stream_ref: SysFSEventStreamRef,
    info: *mut c_void,
    num_events: usize,                           // size_t numEvents
    event_paths: *mut c_void,                    // void *eventPaths
    event_flags: *const FSEventStreamEventFlags, // const FSEventStreamEventFlags eventFlags[]
    event_ids: *const FSEventStreamEventId,      // const FSEventStreamEventId eventIds[]
) {
    drop(catch_unwind(move || {
        callback_impl(
            stream_ref,
            info,
            num_events,
            event_paths,
            event_flags,
            event_ids,
        );
    }));
}

enum CallbackError {
    ToI64,
    ParseFlags,
}

fn event_iter(
    create_flags: FSEventStreamCreateFlags,
    num: usize,
    paths: *mut c_void,
    flags: *const FSEventStreamEventFlags,
    ids: *const FSEventStreamEventId,
) -> impl Iterator<Item = Result<Event, CallbackError>> {
    if create_flags.contains(kFSEventStreamCreateFlagUseCFTypes) {
        Either::Left(
            if create_flags.contains(kFSEventStreamCreateFlagUseExtendedData) {
                // CFDict
                let paths = unsafe { CFArray::<CFDictionary<CFString>>::from_void(paths) };
                Either::Left((0..num).map(move |idx| {
                    Ok((
                        unsafe { paths.get_unchecked(idx as CFIndex) },
                        unsafe { *flags.add(idx) },
                        unsafe { *ids.add(idx) },
                    ))
                    .and_then(|(dict, flags, id)| {
                        if create_flags.contains(kFSEventStreamCreateFlagFileEvents) {
                            // DataPathKey & FileIDKey
                            Ok(Event {
                                path: PathBuf::from(
                                    (*unsafe {
                                        CFString::from_void(
                                            *dict.get(&*kFSEventStreamEventExtendedDataPathKey),
                                        )
                                    })
                                    .to_string(),
                                ),
                                inode: Some(
                                    unsafe {
                                        CFNumber::from_void(
                                            *dict.get(&*kFSEventStreamEventExtendedFileIDKey),
                                        )
                                    }
                                    .to_i64()
                                    .ok_or(CallbackError::ToI64)?,
                                ),
                                flags: StreamFlags::from_bits(flags)
                                    .ok_or(CallbackError::ParseFlags)?,
                                raw_flags: flags,
                                id,
                            })
                        } else {
                            // DataPathKey
                            Ok(Event {
                                path: PathBuf::from(
                                    (*unsafe {
                                        CFString::from_void(
                                            *dict.get(&*kFSEventStreamEventExtendedDataPathKey),
                                        )
                                    })
                                    .to_string(),
                                ),
                                inode: None,
                                flags: StreamFlags::from_bits(flags)
                                    .ok_or(CallbackError::ParseFlags)?,
                                raw_flags: flags,
                                id,
                            })
                        }
                    })
                }))
            } else {
                // CFString
                let paths = unsafe { CFArray::<CFString>::from_void(paths) };
                Either::Right((0..num).map(move |idx| {
                    Ok((
                        unsafe { paths.get_unchecked(idx as CFIndex) },
                        unsafe { *flags.add(idx) },
                        unsafe { *ids.add(idx) },
                    ))
                    .and_then(|(path, flags, id)| {
                        Ok(Event {
                            path: PathBuf::from((*path).to_string()),
                            inode: None,
                            flags: StreamFlags::from_bits(flags)
                                .ok_or(CallbackError::ParseFlags)?,
                            raw_flags: flags,
                            id,
                        })
                    })
                }))
            },
        )
    } else {
        // Normal types
        let paths = paths as *const *const c_char;
        Either::Right((0..num).map(move |idx| {
            Ok((
                unsafe { *paths.add(idx) },
                unsafe { *flags.add(idx) },
                unsafe { *ids.add(idx) },
            ))
            .and_then(|(path, flags, id)| {
                Ok(Event {
                    path: PathBuf::from(
                        OsStr::from_bytes(unsafe { CStr::from_ptr(path) }.to_bytes())
                            .to_os_string(),
                    ),
                    inode: None,
                    flags: StreamFlags::from_bits(flags).ok_or(CallbackError::ParseFlags)?,
                    raw_flags: flags,
                    id,
                })
            })
        }))
    }
}

fn callback_impl(
    _stream_ref: SysFSEventStreamRef,
    info: *mut c_void,
    num_events: usize,                           // size_t numEvents
    event_paths: *mut c_void,                    // void *eventPaths
    event_flags: *const FSEventStreamEventFlags, // const FSEventStreamEventFlags eventFlags[]
    event_ids: *const FSEventStreamEventId,      // const FSEventStreamEventId eventIds[]
) {
    debug!("Received {} event(s)", num_events);

    let info = info as *const StreamContextInfo;
    let create_flags = unsafe { &(*info).create_flags };
    let event_handler = unsafe { &(*info).event_handler };

    let events = event_iter(
        *create_flags,
        num_events,
        event_paths,
        event_flags,
        event_ids,
    )
    .filter_map(|event| {
        if let Err(e) = &event {
            match e {
                CallbackError::ToI64 => error!("Unable to convert inode field to i64"),
                CallbackError::ParseFlags => error!("Unable to parse flags"),
            }
        }
        event.ok()
    })
    .collect();

    if let Err(e) = event_handler.try_send(events) {
        error!("Unable to send event from callback: {}", e);
    }
}
