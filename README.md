# fsevent-stream

[![FOSSA Status](https://app.fossa.com/api/projects/git%2Bgithub.com%2FPhotonQuantum%2Ffsevent-better.svg?type=shield)](https://app.fossa.com/projects/git%2Bgithub.com%2FPhotonQuantum%2Ffsevent-better?ref=badge_shield)

Stream-based FSEvent API bindings.

## Features

- Support directory-granular and file-granular events.
- Retrieve related file inode with `kFSEventStreamCreateFlagUseExtendedData`.

## Runtime Support

Both [`tokio`](https://github.com/tokio-rs/tokio) and [`async-std`](https://github.com/async-rs/async-std) are supported
via feature flags.

`tokio` support is enabled by default. To enable `async-std` support, disable default features and enable `async-std`
feature.

## Acknowledgement

Some code in this project is adapted from the following projects:

- [fsevent-sys](https://github.com/octplane/fsevent-rust)
- [notify](https://github.com/notify-rs/notify)

## License

This project is licensed under [MIT License](LICENSE).

[![FOSSA Status](https://app.fossa.com/api/projects/git%2Bgithub.com%2FPhotonQuantum%2Ffsevent-better.svg?type=large)](https://app.fossa.com/projects/git%2Bgithub.com%2FPhotonQuantum%2Ffsevent-better?ref=badge_large)