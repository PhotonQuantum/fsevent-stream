//! Watcher implementation for Darwin's FSEvents API
//!
//! The FSEvents API provides a mechanism to notify clients about directories they ought to re-scan
//! in order to keep their internal data structures up-to-date with respect to the true state of
//! the file system. (For example, when files or directories are created, modified, or removed.) It
//! sends these notifications "in bulk", possibly notifying the client of changes to several
//! directories in a single callback.
//!
//! For more information see the [FSEvents API reference][ref].
//!
//! [ref]: https://developer.apple.com/library/mac/documentation/Darwin/Reference/FSEvents_Ref/

#![allow(non_upper_case_globals, dead_code)]

use std::ffi::{c_void, CStr};
use std::io;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

use core_foundation::runloop::{kCFRunLoopBeforeWaiting, kCFRunLoopDefaultMode, CFRunLoop};
use futures::stream::{abortable, AbortHandle, Abortable};
use tokio_stream::wrappers::ReceiverStream;

use crate::flags::StreamFlags;
use crate::impl_release_callback;
use crate::observer::create_oneshot_observer;
use crate::raw as fs;
use crate::raw::{
    CFRunLoopExt, FSEventStream, FSEventStreamContext, FSEventStreamCreateFlags,
    FSEventStreamEventFlags, FSEventStreamEventId,
};

/// An owned permission to stop a RawEventStream and terminate its backing RunLoop.
///
/// A `RawEventStreamHandler` *detaches* the associated Stream and RunLoop when it is dropped, which
/// means that there is no longer any handle to them and no way to `abort` them, which may cause
/// memory leaks.
pub struct RawEventStreamHandler {
    runloop: Option<(CFRunLoop, thread::JoinHandle<()>, AbortHandle)>,
}

impl RawEventStreamHandler {
    /// Stop a RawEventStream and terminate its backing RunLoop.
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

#[derive(Debug, Clone)]
pub struct RawEvent {
    path: PathBuf,
    flags: StreamFlags,
    raw_flags: FSEventStreamEventFlags,
    id: FSEventStreamEventId,
}

pub struct RawEventStream {
    stream: Abortable<ReceiverStream<RawEvent>>,
}

struct StreamContextInfo {
    event_handler: tokio::sync::mpsc::Sender<RawEvent>,
}

impl_release_callback!(release_context, StreamContextInfo);

struct SendWrapper<T>(T);

unsafe impl<T> Send for SendWrapper<T> {}

impl<T> SendWrapper<T> {
    unsafe fn new(t: T) -> Self {
        SendWrapper(t)
    }
}

pub fn raw_event_stream<P: AsRef<Path>>(
    paths_to_watch: impl IntoIterator<Item = P>,
    since_when: FSEventStreamEventId,
    latency: Duration,
    flags: FSEventStreamCreateFlags,
) -> io::Result<(RawEventStream, RawEventStreamHandler)> {
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(1024);

    // We need to associate the stream context with our callback in order to propagate events
    // to the rest of the system. This will be owned by the stream, and will be freed when the
    // stream is closed. This means we will leak the context if we panic before reacing
    // `FSEventStreamRelease`.
    let context = StreamContextInfo {
        event_handler: event_tx,
    };

    let stream_context = FSEventStreamContext::new(context, release_context);

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
        let current_runloop = CFRunLoop::get_current();

        stream.schedule(&current_runloop, unsafe { kCFRunLoopDefaultMode });
        stream.start();

        // the calling to CFRunLoopRun will be terminated by CFRunLoopStop call in drop()
        // SAFETY: `CF_REF` is thread-safe.
        runloop_tx
            .send(unsafe { SendWrapper::new(current_runloop) })
            .expect("Unable to send runloop to watcher");

        CFRunLoop::run_current();
        stream.stop();
        stream.invalidate();
    });

    let (stream, stream_handle) = abortable(ReceiverStream::new(event_rx));
    Ok((
        RawEventStream { stream },
        RawEventStreamHandler {
            runloop: Some((runloop_rx.recv().unwrap().0, thread_handle, stream_handle)),
        },
    ))
}

extern "C" fn callback(
    stream_ref: fs::FSEventStreamRef,
    info: *mut c_void,
    num_events: usize,                               // size_t numEvents
    event_paths: *mut c_void,                        // void *eventPaths
    event_flags: *const fs::FSEventStreamEventFlags, // const FSEventStreamEventFlags eventFlags[]
    event_ids: *const fs::FSEventStreamEventId,      // const FSEventStreamEventId eventIds[]
) {
    unsafe {
        callback_impl(
            stream_ref,
            info,
            num_events,
            event_paths,
            event_flags,
            event_ids,
        )
    }
}

unsafe fn callback_impl(
    _stream_ref: fs::FSEventStreamRef,
    info: *mut c_void,
    num_events: usize,                               // size_t numEvents
    event_paths: *mut c_void,                        // void *eventPaths
    event_flags: *const fs::FSEventStreamEventFlags, // const FSEventStreamEventFlags eventFlags[]
    event_ids: *const fs::FSEventStreamEventId,      // const FSEventStreamEventId eventIds[]
) {
    let event_paths = event_paths as *const *const c_char;
    let info = info as *const StreamContextInfo;
    let event_handler = &(*info).event_handler;

    for idx in 0..num_events {
        if let Some(raw_event) = Some((
            *event_paths.add(idx),
            *event_flags.add(idx),
            *event_ids.add(idx),
        ))
        .and_then(|(path, raw_flags, id)| {
            CStr::from_ptr(path)
                .to_str()
                .ok()
                .map(|path| (PathBuf::from(path), raw_flags, id))
        })
        .and_then(|(path, raw_flags, id)| {
            StreamFlags::from_bits(raw_flags).map(|flags| RawEvent {
                path,
                flags,
                raw_flags,
                id,
            })
        }) {
            // Send event out.
            drop(event_handler.send(raw_event));
        }
    }
}

#[test]
fn test_steam_context_info_send_and_sync() {
    fn check_send<T: Send + Sync>() {}
    check_send::<StreamContextInfo>();
}
