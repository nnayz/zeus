set shell := ["bash", "-uc"]

# List the available recipes.
default:
    @just --list

# Compile the Rust workspace.
[group('rust')]
build:
    cd taskmaster && cargo build --workspace

# Compile the Rust workspace with the release profile.
[group('rust')]
build-release:
    cd taskmaster && cargo build --workspace --release

# Format the Rust workspace.
[group('rust')]
fmt:
    cd taskmaster && cargo fmt --all

# Verify Rust formatting without rewriting files.
[group('rust')]
fmt-check:
    cd taskmaster && cargo fmt --all -- --check

# Lint the Rust workspace with Clippy.
[group('rust')]
lint:
    cd taskmaster && cargo clippy --workspace --all-targets -- -D warnings

# Run the Rust workspace tests.
[group('rust')]
test:
    cd taskmaster && cargo test --workspace

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
    taskmaster/scripts/dev.sh

# Rebuild and relaunch the development app on changes.
[group('desktop')]
dev-watch:
    taskmaster/scripts/dev.sh --watch

# Build the workbench preview.
[group('desktop')]
dev-ui:
    taskmaster/scripts/dev-ui.sh

# Assemble the packaged taskmaster.app bundle.
[group('desktop')]
package:
    taskmaster/scripts/package.sh

# Install the locally built taskmaster.app bundle.
[group('desktop')]
install:
    taskmaster/scripts/install-local.sh

# Create the stable "Taskmaster Dev" code-signing identity used by dev builds.
[group('desktop')]
dev-cert:
    scripts/make-dev-cert.sh

# Serve the website, user guide, and playbooks locally.
[group('docs')]
docs:
    cd website && npm run dev

# iOS Companion recipes (`just ios`, `just ios build`, `just ios run`, ...).
mod ios

# Prototype recipes (`just prototypes`, `just prototypes test`, ...).
mod prototypes
