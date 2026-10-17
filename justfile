set shell := ["bash", "-uc"]

# List the available recipes.
default:
    @just --list

# Compile the Rust workspace.
[group('rust')]
build:
    cd zeus && cargo build --workspace

# Compile the Rust workspace with the release profile.
[group('rust')]
build-release:
    cd zeus && cargo build --workspace --release

# Format the Rust workspace.
[group('rust')]
fmt:
    cd zeus && cargo fmt --all

# Verify Rust formatting without rewriting files.
[group('rust')]
fmt-check:
    cd zeus && cargo fmt --all -- --check

# Lint the Rust workspace with Clippy.
[group('rust')]
lint:
    cd zeus && cargo clippy --workspace --all-targets -- -D warnings

# Run the Rust workspace tests.
[group('rust')]
test:
    cd zeus && cargo test --workspace

# Run the repository's CI-equivalent checks.
[group('checks')]
check:
    ./scripts/check.sh

# Run the checks plus the browser sidecar integration tests.
[group('checks')]
check-browser:
    ./scripts/check.sh --browser

# Build and launch a development copy of the desktop app.
[group('desktop')]
dev:
    zeus/scripts/dev.sh

# Rebuild and relaunch the development app on changes.
[group('desktop')]
dev-watch:
    zeus/scripts/dev.sh --watch

# Build the workbench preview.
[group('desktop')]
dev-ui:
    zeus/scripts/dev-ui.sh

# Assemble the packaged zeus.app bundle.
[group('desktop')]
package:
    zeus/scripts/package.sh

# Install the locally built zeus.app bundle.
[group('desktop')]
install:
    zeus/scripts/install-local.sh

# Create the stable "Zeus Dev" code-signing identity used by dev builds.
[group('desktop')]
dev-cert:
    scripts/make-dev-cert.sh

# Serve the user guide locally with mdBook.
[group('docs')]
docs:
    mdbook serve docs

# iOS Companion recipes (`just ios`, `just ios build`, `just ios run`, ...).
mod ios

# Prototype recipes (`just prototypes`, `just prototypes test`, ...).
mod prototypes
