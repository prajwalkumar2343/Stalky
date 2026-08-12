# syntax=docker/dockerfile:1

# ----------------------------------------------------------------------------
# Stalky backend image.
#
# Multi-stage build:
#   1. builder  - pinned Rust toolchain, compiles the workspace member
#                 `stalky-backend`.
#   2. runtime  - minimal non-root distroless image containing only the binary.
#
# The backend is a member of a Cargo workspace and reads workspace-level
# settings (edition/version and workspace dependencies), so the build context
# MUST be the repository root and the full workspace is copied. Only the backend
# crate is compiled; the desktop/platform crates are not built here.
#
# Build (from the repository root):
#   docker build -f backend/Dockerfile -t stalky-backend:latest .
#
# Run:
#   docker run --rm -p 8080:8080 \
#     -e SUPABASE_URL=https://<project>.supabase.co \
#     -e RUST_LOG=info \
#     stalky-backend:latest
# ----------------------------------------------------------------------------

FROM rust:1.88.0-bookworm AS builder

WORKDIR /app

# Workspace root manifests first so dependency resolution can be cached alone.
COPY Cargo.toml Cargo.lock ./

# The backend crate itself.
COPY backend ./backend

# Copy the remaining workspace manifests/sources so Cargo can load every member
# declared by the workspace root (required even when only the backend builds).
COPY apps ./apps
COPY crates ./crates

# --locked makes the build fail instead of diverging from Cargo.lock.
RUN cargo build --release --manifest-path backend/Cargo.toml --locked

# ----------------------------------------------------------------------------
# Runtime: non-root, no shell, glibc-compatible minimal image.
# The distroless `nonroot` variant runs as UID 65532 by default.
# ----------------------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot

WORKDIR /app

COPY --from=builder /app/target/release/stalky-backend /usr/local/bin/stalky-backend

USER nonroot

# Defaults are overridable via the environment; kept here so bare `docker run`
# binds on all interfaces inside the container.
ENV BIND_ADDRESS=0.0.0.0:8080

EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/stalky-backend"]
