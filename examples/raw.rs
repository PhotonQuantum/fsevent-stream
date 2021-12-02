use std::path::Path;
use std::time::Duration;

use tokio_stream::StreamExt;

use fsevent_better::fsevent::raw_event_stream;
use fsevent_better::raw::{
    kFSEventStreamCreateFlagFileEvents, kFSEventStreamCreateFlagNoDefer,
    kFSEventStreamEventIdSinceNow,
};

#[tokio::main]
async fn main() {
    run().await;
}

async fn run() {
    let (mut stream, _handler) = raw_event_stream(
        [Path::new("../")],
        kFSEventStreamEventIdSinceNow,
        Duration::ZERO,
        kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer,
    )
    .expect("stream to be created");
    while let Some(raw_event) = stream.next().await {
        println!(
            "[{}] path: {:?}, flags: {} ({:x})",
            raw_event.id, raw_event.path, raw_event.flags, raw_event.raw_flags
        );
    }
}
