# syntax=docker/dockerfile:1.7
# Cross-builds the Windows binaries on Linux — no Rust / Visual Studio needed on the host.
#
#   docker build --target export --output dist .
#
# Result in ./dist:
#   launcherd.exe                     headless server (UI in browser)
#   LlamaLauncher-portable.exe        desktop app (window + tray), needs WebView2 (built into Win10/11)
#   Llama Launcher_<ver>_x64-setup.exe  NSIS installer
#
# MSVC CRT/Windows SDK headers are fetched by cargo-xwin (xwin); by building you accept
# the Microsoft license: https://go.microsoft.com/fwlink/?LinkId=2086102

ARG RUST_VERSION=1
ARG NODE_VERSION=22

# ---------- UI (Svelte) ----------
FROM node:${NODE_VERSION}-bookworm-slim AS ui
WORKDIR /src/ui
COPY ui/package.json ui/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci
COPY ui/ ./
RUN npm run build

# ---------- toolchain (cached as a layer, rebuilt only when this block changes) ----------
# trixie: NSIS >= 3.09 (Win\RestartManager.nsh) required by the Tauri installer template
FROM rust:${RUST_VERSION}-trixie AS toolchain
# appindicator: tauri-cli checks for it on the (Linux) host when the app uses tray-icon
RUN apt-get update \
 && apt-get install -y --no-install-recommends clang lld llvm nsis libayatana-appindicator3-dev \
 && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-pc-windows-msvc
RUN cargo install --locked cargo-xwin \
 && cargo install --locked tauri-cli --version "^2"
ENV XWIN_ACCEPT_LICENSE=1 \
    XWIN_CACHE_DIR=/xwin-cache \
    CARGO_TERM_COLOR=always

# ---------- build ----------
FROM toolchain AS build
WORKDIR /src
COPY . .
COPY --from=ui /src/ui/dist ./ui/dist
ARG TARGET=x86_64-pc-windows-msvc
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/xwin-cache \
    --mount=type=cache,target=/src/target \
    set -eux; \
    cargo xwin build --release -p launcherd --target $TARGET; \
    (cd app && cargo tauri build --runner cargo-xwin --target $TARGET --bundles nsis); \
    mkdir -p /out; \
    cp target/$TARGET/release/launcherd.exe /out/; \
    cp target/$TARGET/release/llama-launcher.exe /out/LlamaLauncher-portable.exe; \
    cp target/$TARGET/release/bundle/nsis/*.exe /out/

# ---------- export only the artifacts ----------
FROM scratch AS export
COPY --from=build /out/ /
