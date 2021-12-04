use std::path::Path;
use std::time::Duration;

#[cfg(feature = "async-std")]
use async_std1 as async_std;
use futures_util::StreamExt;
use log::info;
#[cfg(feature = "tokio")]
use tokio1 as tokio;

use fsevent_better::ffi::{
    kFSEventStreamCreateFlagFileEvents, kFSEventStreamCreateFlagNoDefer,
    kFSEventStreamCreateFlagNone, kFSEventStreamCreateFlagUseCFTypes,
    kFSEventStreamCreateFlagUseExtendedData, kFSEventStreamEventIdSinceNow,
};
use fsevent_better::stream::create_event_stream;

#[cfg(feature = "tokio")]
#[tokio::main]
async fn main() {
    run().await;
}

#[cfg(feature = "async-std")]
#[async_std::main]
async fn main() {
    run().await;
}

async fn run() {
    pretty_env_logger::init();
    let (mut stream, _handler) = create_event_stream(
        [Path::new(".")],
        kFSEventStreamEventIdSinceNow,
        Duration::from_secs(5),
        kFSEventStreamCreateFlagNoDefer
            | kFSEventStreamCreateFlagFileEvents
            | kFSEventStreamCreateFlagUseExtendedData
            | kFSEventStreamCreateFlagUseCFTypes,
    )
    .expect("stream to be created");
    while let Some(raw_event) = stream.next().await {
        info!(
            "[{}] path: {:?}({}), flags: {} ({:x})",
            raw_event.id,
            raw_event.path,
            raw_event.inode.unwrap_or(-1),
            raw_event.flags,
            raw_event.raw_flags
        );
    }
}
