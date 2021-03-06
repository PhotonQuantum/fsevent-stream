use std::path::Path;
use std::time::Duration;

#[cfg(feature = "async-std")]
use async_std1 as async_std;
use futures_util::StreamExt;
use log::info;
#[cfg(feature = "tokio")]
use tokio1 as tokio;

use fsevent_stream::ffi::{
    kFSEventStreamCreateFlagFileEvents, kFSEventStreamCreateFlagNoDefer,
    kFSEventStreamCreateFlagUseCFTypes, kFSEventStreamCreateFlagUseExtendedData,
    kFSEventStreamEventIdSinceNow,
};
use fsevent_stream::stream::create_event_stream;

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
    let (stream, _handler) = create_event_stream(
        [Path::new("./")],
        kFSEventStreamEventIdSinceNow,
        Duration::ZERO,
        kFSEventStreamCreateFlagNoDefer
            | kFSEventStreamCreateFlagFileEvents
            | kFSEventStreamCreateFlagUseExtendedData
            | kFSEventStreamCreateFlagUseCFTypes,
    )
    .expect("stream to be created");
    let mut stream = stream.into_flatten();
    while let Some(event) = stream.next().await {
        info!("{}", event);
    }
}
