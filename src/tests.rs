#![allow(clippy::borrow_interior_mutable_const, clippy::cast_possible_wrap)]

use std::fs;
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::Ordering;
use std::sync::mpsc::channel;
use std::thread;
use std::thread::sleep;
use std::time::Duration;

use futures::StreamExt;
use once_cell::sync::Lazy;
use tempfile::tempdir;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::ffi::{
    kFSEventStreamCreateFlagFileEvents, kFSEventStreamCreateFlagNoDefer,
    kFSEventStreamCreateFlagNone, kFSEventStreamCreateFlagUseCFTypes,
    kFSEventStreamCreateFlagUseExtendedData, kFSEventStreamEventIdSinceNow,
    FSEventStreamCreateFlags,
};
use crate::stream::{
    create_event_stream, StreamContextInfo, StreamFlags, TEST_RUNNING_RUNLOOP_COUNT,
};

static TEST_PARALLEL_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[test]
fn must_steam_context_info_send_and_sync() {
    fn check_send<T: Send + Sync>() {}
    check_send::<StreamContextInfo>();
}

#[tokio::test]
async fn must_abort_stream() {
    // Acquire the lock so that no other runloop can be created during this test.
    let _guard = TEST_PARALLEL_LOCK.lock().await;

    // Create the stream to be tested.
    let (stream, mut handler) = create_event_stream(
        ["."],
        kFSEventStreamEventIdSinceNow,
        Duration::ZERO,
        kFSEventStreamCreateFlagNone,
    )
    .expect("to be created");
    // Now there should be one runloop.
    assert_eq!(TEST_RUNNING_RUNLOOP_COUNT.load(Ordering::SeqCst), 1);

    // Abort the stream immediately.
    let abort_thread = thread::spawn(move || {
        handler.abort();
    });

    // The stream should complete soon.
    drop(
        timeout(Duration::from_secs(1), stream.collect::<Vec<_>>())
            .await
            .expect("to complete"),
    );
    // The runloop should be released.
    assert_eq!(TEST_RUNNING_RUNLOOP_COUNT.load(Ordering::SeqCst), 0);

    abort_thread.join().expect("to join");
}

#[tokio::test]
#[allow(clippy::semicolon_if_nothing_returned)]
async fn must_receive_fs_events() {
    // Acquire the lock so that runloop created in this test won't affect others.
    let _guard = TEST_PARALLEL_LOCK.lock().await;

    let _ = tokio::join!(
        must_receive_fs_events_impl(
            kFSEventStreamCreateFlagFileEvents
                | kFSEventStreamCreateFlagUseCFTypes
                | kFSEventStreamCreateFlagUseExtendedData,
            true,
            true,
        ),
        must_receive_fs_events_impl(
            kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagUseCFTypes,
            false,
            true,
        ),
        must_receive_fs_events_impl(kFSEventStreamCreateFlagFileEvents, false, true),
        must_receive_fs_events_impl(
            kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagUseExtendedData,
            false,
            false,
        ),
        must_receive_fs_events_impl(kFSEventStreamCreateFlagUseCFTypes, false, false)
    );
}

async fn must_receive_fs_events_impl(
    flags: FSEventStreamCreateFlags,
    verify_inode: bool,
    verify_file_events: bool,
) {
    // Create the test dir.
    let dir = tempdir().expect("to be created");
    let test_file = dir
        .path()
        .canonicalize() // ensure it's an canonical path because FSEvent api returns that
        .expect("to succeed")
        .join("test_file");

    // Create a channel to inform the abort thread that fs operations are completed.
    let (tx, rx) = channel();

    // Create the stream to be tested.
    let (stream, mut handler) = create_event_stream(
        [dir.path()],
        kFSEventStreamEventIdSinceNow,
        Duration::ZERO,
        flags | kFSEventStreamCreateFlagNoDefer,
    )
    .expect("to be created");
    let abort_thread = thread::spawn(move || {
        // Once fs operations are completed, abort the stream.
        rx.recv().expect("to be signaled");
        // Tolerance time
        if option_env!("CI").is_some() {
            sleep(Duration::from_secs(5));
        }
        handler.abort();
    });

    // First we create a file.
    let f = File::create(&test_file).expect("to be created");
    let inode = f.metadata().expect("to be fetched").ino() as i64;
    // Sync so that ITEM_CREATE and ITEM_DELETE events won't be squashed into one.
    f.sync_all().expect("to succeed");
    drop(f);
    // Now we delete this file.
    fs::remove_file(&test_file).expect("to be removed");
    // Ensure the filesystem is up to date.
    unsafe { libc::sync() };
    // Signal the abort thread that we are ready.
    tx.send(()).expect("to signal");

    // It's fine to consume the stream later because it's reactive and can still be consumed if it's aborted.
    let events: Vec<_> = timeout(Duration::from_secs(10), stream.collect())
        .await
        .expect("to complete");

    if verify_file_events {
        // A dir creation event might be recorded so it's ok we receive 2~3 events.
        assert!(events.len() == 2 || events.len() == 3);

        // The second last event should be the file creation event.
        let event_fst = events.get(events.len() - 2).expect("to exist");
        assert_eq!(event_fst.path.as_path(), test_file.as_path());
        if verify_inode {
            assert_eq!(event_fst.inode, Some(inode));
        }
        assert!(event_fst
            .flags
            .contains(StreamFlags::ITEM_CREATED | StreamFlags::IS_FILE));

        // The last event should be the file deletion event.
        let event_snd = events.last().expect("to exist");
        assert_eq!(event_snd.path.as_path(), test_file.as_path());
        if verify_inode {
            assert_eq!(event_snd.inode, Some(inode));
        }
        assert!(event_snd
            .flags
            .contains(StreamFlags::ITEM_REMOVED | StreamFlags::IS_FILE));
    } else {
        assert!(!events.is_empty());
    }

    abort_thread.join().expect("to join");
}
