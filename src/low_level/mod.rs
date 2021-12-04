//! Low-level stream-based `FSEvents` interface.
#![allow(
    clippy::borrow_interior_mutable_const,
    clippy::cast_possible_wrap,
    clippy::non_send_fields_in_send_ty
)]

use std::ffi::{c_void, CStr, OsStr};
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

use core_foundation::array::CFArray;
use core_foundation::base::{CFIndex, FromVoid};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::runloop::{kCFRunLoopBeforeWaiting, kCFRunLoopDefaultMode, CFRunLoop};
use core_foundation::string::CFString;
use either::Either;
use futures::stream::{abortable, AbortHandle, Abortable};
use futures::{Stream, StreamExt};
use log::{debug, error};
use tokio_stream::wrappers::ReceiverStream;

pub use flags::StreamFlags;

use crate::impl_release_callback;
use crate::observer::create_oneshot_observer;
use crate::sys::{
    kFSEventStreamCreateFlagFileEvents, kFSEventStreamCreateFlagUseCFTypes,
    kFSEventStreamCreateFlagUseExtendedData, kFSEventStreamEventExtendedDataPathKey,
    kFSEventStreamEventExtendedFileIDKey, CFRunLoopExt, FSEventStream, FSEventStreamContext,
    FSEventStreamCreateFlags, FSEventStreamEventFlags, FSEventStreamEventId, FSEventStreamRef,
};
use crate::utils::FlagsExt;

mod flags;
#[cfg(test)]
mod tests;

/// An owned permission to stop a `RawEventStream` and terminate its backing `RunLoop`.
///
/// A `RawEventStreamHandler` *detaches* the associated Stream and `RunLoop` when it is dropped, which
/// means that there is no longer any handle to them and no way to `abort` them.
///
/// Dropping the handler without first calling [`abort`](RawEventStreamHandler::abort) is not
/// recommended because this leaves a spawned thread behind and causes memory leaks.
pub struct RawEventStreamHandler {
    runloop: Option<(CFRunLoop, thread::JoinHandle<()>, AbortHandle)>,
}

// Safety:
// - According to the Apple documentation, it's safe to move `CFRef`s across threads.
//   https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Multithreading/ThreadSafetySummary/ThreadSafetySummary.html
unsafe impl Send for RawEventStreamHandler {}

impl RawEventStreamHandler {
    /// Stop a `RawEventStream` and terminate its backing `RunLoop`.
    ///
    /// Calling this method multiple times has no extra effect and won't cause any panic, error,
    /// or undefined behavior.
    pub fn abort(&mut self) {
        if let Some((runloop, thread_handle, abort_handle)) = self.runloop.take() {
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

            // Abort the stream.
            abort_handle.abort();
        }
    }
}

/// A low-level `FSEvents` event.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct RawEvent {
    pub path: PathBuf,
    pub inode: Option<i64>,
    pub flags: StreamFlags,
    pub raw_flags: FSEventStreamEventFlags,
    pub id: FSEventStreamEventId,
}

/// A stream of low-level `FSEvents` API events.
///
/// Call [`raw_event_stream`](raw_event_stream) to create it.
pub struct RawEventStream {
    stream: Abortable<ReceiverStream<RawEvent>>,
}

impl Stream for RawEventStream {
    type Item = RawEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.poll_next_unpin(cx)
    }
}

struct StreamContextInfo {
    event_handler: tokio::sync::mpsc::Sender<RawEvent>,
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

/// Create a new [`RawEventStream`](RawEventStream) and [`RawEventStreamHandler`](RawEventStreamHandler) pair.
///
/// # Errors
/// Return error when there's any invalid path in `paths_to_watch`.
///
/// # Panics
/// Panic when the given flags combination is illegal.
pub fn raw_event_stream<P: AsRef<Path>>(
    paths_to_watch: impl IntoIterator<Item = P>,
    since_when: FSEventStreamEventId,
    latency: Duration,
    flags: FSEventStreamCreateFlags,
) -> io::Result<(RawEventStream, RawEventStreamHandler)> {
    if flags.contains(kFSEventStreamCreateFlagUseExtendedData)
        && !flags.contains(kFSEventStreamCreateFlagUseCFTypes)
    {
        panic!("UseExtendedData requires UseCFTypes");
    }

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(1024);

    // We need to associate the stream context with our callback in order to propagate events
    // to the rest of the system. This will be owned by the stream, and will be freed when the
    // stream is closed. This means we will leak the context if we panic before reacing
    // `FSEventStreamRelease`.
    let context = StreamContextInfo {
        event_handler: event_tx,
        create_flags: flags,
    };

    let stream_context = FSEventStreamContext::new(context, release_context);

    // We must append some additional flags because our callback parse them so
    let mut stream = FSEventStream::new(
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

    let (stream, stream_handle) = abortable(ReceiverStream::new(event_rx));
    Ok((
        RawEventStream { stream },
        RawEventStreamHandler {
            runloop: Some((
                runloop_rx.recv().expect("receive runloop from worker").0,
                thread_handle,
                stream_handle,
            )),
        },
    ))
}

#[cfg(test)]
static TEST_RUNNING_RUNLOOP_COUNT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

extern "C" fn callback(
    stream_ref: FSEventStreamRef,
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
) -> impl Iterator<Item = Result<RawEvent, CallbackError>> {
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
                            Ok(RawEvent {
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
                            Ok(RawEvent {
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
                        Ok(RawEvent {
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
                Ok(RawEvent {
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
    _stream_ref: FSEventStreamRef,
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

    for event in event_iter(
        *create_flags,
        num_events,
        event_paths,
        event_flags,
        event_ids,
    ) {
        match event {
            Ok(event) => {
                if let Err(e) = event_handler.try_send(event) {
                    error!("Unable to raw event from low-level callback: {}", e);
                }
            }
            Err(CallbackError::ToI64) => error!("Unable to convert inode field to i64"),
            Err(CallbackError::ParseFlags) => error!("Unable to parse flags"),
        }
    }
}
