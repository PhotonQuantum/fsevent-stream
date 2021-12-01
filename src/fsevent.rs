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
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop};

use crate::events::EventHandler;
use crate::flags::StreamFlags;
use crate::impl_release_callback;
use crate::raw as fs;
use crate::raw::{
    CFRunLoopExt, FSEventStream, FSEventStreamContext, FSEventStreamCreateFlags,
    FSEventStreamEventId,
};

/// FSEvents-based `Watcher` implementation
pub struct FsEventWatcher {
    runloop: Option<(CFRunLoop, thread::JoinHandle<()>)>,
}

// unsafe impl Send for FsEventWatcher {}
// unsafe impl Sync for FsEventWatcher {}

struct StreamContextInfo {
    event_handler: Arc<Mutex<dyn EventHandler>>,
}

impl_release_callback!(release_context, StreamContextInfo);

struct SendWrapper<T>(T);

unsafe impl<T> Send for SendWrapper<T> {}

impl<T> SendWrapper<T> {
    unsafe fn new(t: T) -> Self {
        SendWrapper(t)
    }
}

impl FsEventWatcher {
    fn stop(&mut self) {
        if let Some((runloop, thread_handle)) = self.runloop.take() {
            while !runloop.is_waiting() {
                thread::yield_now();
            }
            runloop.stop();

            // Wait for the thread to shut down.
            thread_handle.join().expect("thread to shut down");
        }
    }

    fn new<P: AsRef<Path>>(
        event_handler: impl EventHandler,
        paths_to_watch: impl IntoIterator<Item = P>,
        since_when: FSEventStreamEventId,
        latency: Duration,
        flags: FSEventStreamCreateFlags,
    ) -> io::Result<Self> {
        // We need to associate the stream context with our callback in order to propagate events
        // to the rest of the system. This will be owned by the stream, and will be freed when the
        // stream is closed. This means we will leak the context if we panic before reacing
        // `FSEventStreamRelease`.
        let context = StreamContextInfo {
            event_handler: Arc::new(Mutex::new(event_handler)),
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
        let (tx, rx) = channel();

        let thread_handle = thread::spawn(move || {
            let current_runloop = CFRunLoop::get_current();

            stream.schedule(&current_runloop, unsafe { kCFRunLoopDefaultMode });
            stream.start();

            // the calling to CFRunLoopRun will be terminated by CFRunLoopStop call in drop()
            // SAFETY: `CF_REF` is thread-safe.
            tx.send(unsafe { SendWrapper::new(current_runloop) })
                .expect("Unable to send runloop to watcher");

            CFRunLoop::run_current();
            stream.stop();
            stream.invalidate();
        });

        Ok(Self {
            runloop: Some((rx.recv().unwrap().0, thread_handle)),
        })
    }
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
    _event_ids: *const fs::FSEventStreamEventId,     // const FSEventStreamEventId eventIds[]
) {
    let event_paths = event_paths as *const *const c_char;
    let info = info as *const StreamContextInfo;
    let event_handler = &(*info).event_handler;

    for p in 0..num_events {
        let path = CStr::from_ptr(*event_paths.add(p))
            .to_str()
            .expect("Invalid UTF8 string.");
        let path = PathBuf::from(path);

        let flag = *event_flags.add(p);
        let flag = StreamFlags::from_bits(flag).unwrap_or_else(|| {
            panic!("Unable to decode StreamFlags: {}", flag);
        });

        // for ev in translate_flags(flag, true).into_iter() {
        //     let ev = ev.add_path(path.clone());
        //     let mut event_handler = event_handler.lock().expect("lock not to be poisoned");
        //     event_handler.handle_event(ev);
        // }
    }
}

impl Drop for FsEventWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
fn test_steam_context_info_send_and_sync() {
    fn check_send<T: Send + Sync>() {}
    check_send::<StreamContextInfo>();
}
