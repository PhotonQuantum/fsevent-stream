ci: fmt clippy-all

fmt:
	cargo fmt -- --check

test feature:
	cargo test --no-default-features --features "{{feature}}"

test-all: (test "tokio") (test "async-std")

clippy feature:
	cargo clippy --all --no-default-features --features "{{feature}}" -- -W clippy::all -W clippy::nursery -W clippy::pedantic
	cargo clippy --all --tests --no-default-features --features "{{feature}}" -- -W clippy::all -W clippy::nursery -W clippy::pedantic

clippy-all: (clippy "tokio") (clippy "async-std")