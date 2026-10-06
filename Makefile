.PHONY: build install run clean

build:
	cargo build --release

# Binaries are enumerated by ccebuild from cargo metadata — never name one here.
install: build
	@command -v ccebuild >/dev/null || { echo "ccebuild not installed — run: make -C ../cce-compositor install"; exit 1; }
	ccebuild install --no-build cce-weather

run:
	cargo run

clean:
	cargo clean
