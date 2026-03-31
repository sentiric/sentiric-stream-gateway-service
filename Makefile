.PHONY: all fmt clippy test build run

all: fmt clippy test build

fmt:
	cargo fmt --all

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test --all-features

build:
	cargo build --release

run:
	cargo run