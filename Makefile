.PHONY: fmt lint test check coverage coverage-summary bench sdk-test docker scan

fmt:
	cargo fmt --all

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

check: lint test

coverage:
	cargo llvm-cov --workspace --lcov --output-path lcov.info

coverage-summary:
	cargo llvm-cov --workspace --summary-only

bench:
	cargo run -p mnemoa-bench --release

sdk-test:
	cd sdk/python && python -m pytest -q

docker:
	docker compose build

scan:
	cargo run -q -p mnemoa-cli -- scan .
