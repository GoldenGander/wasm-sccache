---
name: docker-forked-sccache
description: Teach another agent how to install and use a forked or branch-specific sccache build inside a Dockerfile instead of the normal upstream/package-manager sccache. Use when a project must consume custom sccache behavior from a local fork, personal public GitHub repo, branch, or commit, especially for Emscripten or other unreleased compiler support.
---

# Docker Forked Sccache

Install the custom `sccache` binary from source or from a pinned git revision. Do not tell the user to install the normal package-manager or crates.io `sccache` when they explicitly need fork-only behavior.

## Choose the installation strategy

Prefer a multi-stage Docker build that compiles `sccache` in a builder stage and copies only the resulting binary into the final image.

Use `cargo install --git ... --rev ... --locked sccache` only when a smaller Dockerfile matters more than build speed or reproducibility diagnostics.

Avoid these when the user needs fork-only behavior:

- `cargo install sccache`
- `apt install sccache`
- `brew install sccache`
- downloading the upstream release binary

## Default recommendation

Use a pinned commit SHA from the fork, not a moving branch name.

For personal but public GitHub code, treat it like third-party code:

- Pin `--rev <full_commit_sha>` in Dockerfiles and CI.
- Mention the fork owner, repo, and exact commit in comments or build args.
- Prefer updating by intentional SHA bumps in PRs rather than rebuilding from `main` or `master`.
- If the fork is short-lived and the patch is small, consider merging upstream and carrying a tiny patch in your own repo instead of depending on a personal fork forever.
- If supply-chain review matters, build from a checked-in submodule or vendored source tarball rather than cloning a live branch during image builds.

Inference rule: a personal public fork is acceptable for internal builds if the dependency is pinned immutably and reviewed like any other external dependency.

## Dockerfile pattern

Use this pattern unless the base image already contains a Rust toolchain:

```dockerfile
FROM rust:1.85-bookworm AS sccache-builder

ARG SCCACHE_REPO=https://github.com/<owner>/wasm-sccache
ARG SCCACHE_REV=<full_commit_sha>

RUN cargo install \
    --git ${SCCACHE_REPO} \
    --rev ${SCCACHE_REV} \
    --locked \
    sccache

FROM debian:bookworm-slim

COPY --from=sccache-builder /usr/local/cargo/bin/sccache /usr/local/bin/sccache
ENV SCCACHE_DIR=/var/cache/sccache
```

If the image already builds the main project with Rust, prefer `cargo build --release` in the builder stage so dependency caching is easier to control:

```dockerfile
FROM rust:1.85-bookworm AS sccache-builder
WORKDIR /src
COPY . /src
RUN cargo build --release --bin sccache

FROM debian:bookworm-slim
COPY --from=sccache-builder /src/target/release/sccache /usr/local/bin/sccache
```

Use the second form only when the Docker build context already contains the fork source or when building from a local checkout in CI.

## Emscripten usage

If the project uses `emcc` or `em++`, set the compiler launcher to the custom `sccache` binary and keep the real compiler as `emcc` or `em++`.

Examples:

```dockerfile
ENV CC=emcc
ENV CXX=em++
ENV CMAKE_C_COMPILER_LAUNCHER=/usr/local/bin/sccache
ENV CMAKE_CXX_COMPILER_LAUNCHER=/usr/local/bin/sccache
```

For direct command wrappers:

```dockerfile
ENV SCCACHE_PATH=/usr/local/bin/sccache
RUN $SCCACHE_PATH em++ -c hello.cpp -o hello.o -s WASM=1
```

Do not claim link steps are cacheable if the fork only supports compile-only invocations. Call out that `-c` must be present for cache hits.

## What to tell the user

State these points clearly:

- This installs the forked `sccache`, not the upstream release.
- The Dockerfile is pinned to a specific git revision for reproducibility.
- The compiler remains `emcc` or `em++`; `sccache` is the launcher/wrapper.
- If the fork has not been published to crates.io or upstream releases, the image must build from git or local source.

## Common mistakes

- Installing upstream `sccache` from crates.io and assuming it contains the fork changes.
- Pinning to a branch instead of a commit SHA.
- Replacing `CC`/`CXX` with `sccache` instead of configuring `sccache` as the launcher.
- Expecting link commands without `-c` to be cached.
- Copying an untracked local file into the image build plan and assuming CI can reproduce it.

## Output style

When using this skill, produce:

1. A complete Dockerfile snippet or patch.
2. A short note explaining why the fork is pinned by commit SHA.
3. A short note explaining how the project should point `emcc`/`em++` at `sccache`.
