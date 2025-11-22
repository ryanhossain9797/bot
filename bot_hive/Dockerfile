# Build stage
# Always cross-compile to x86_64 using cross tool
# Note: Builder runs on host platform, but always produces x86_64 binary
FROM rustlang/rust:nightly-bookworm AS builder

# Install build dependencies and cross-compilation toolchain
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libclang-dev \
    cmake \
    build-essential \
    git \
    gcc-x86-64-linux-gnu \
    && rm -rf /var/lib/apt/lists/*

# Install cross tool
RUN cargo install cross

# Add x86_64 target
RUN rustup target add x86_64-unknown-linux-gnu

# Set working directory
WORKDIR /app

# Copy workspace files (build context should be parent directory)
COPY Cargo.toml Cargo.lock ./
COPY lib_hive ./lib_hive
COPY bot_hive ./bot_hive
COPY .git ./.git

# Build the project in release mode for x86_64 using cross
# Use passthrough mode since we're already in Docker
RUN CROSS_DOCKER_IN_DOCKER=false \
    cross build --release --target x86_64-unknown-linux-gnu --package bot

# Capture git commit message
RUN git log -1 --pretty=%B > /app/commit.txt || echo "unknown" > /app/commit.txt

# Runtime stage - uses base image from Docker Hub
# Explicitly use linux/amd64 platform to ensure x86_64 final image
# Replace 'zireael9797' with your actual Docker Hub username
FROM --platform=linux/amd64 zireael9797/bot-base:latest

# Copy binary from builder (only this changes on code updates)
# cross builds to target/x86_64-unknown-linux-gnu/release
COPY --from=builder /app/target/x86_64-unknown-linux-gnu/release/bot /app/bot

# Copy commit message file
COPY --from=builder /app/commit.txt /app/commit.txt

# Note: configuration.rs must be provided at build time
# The DISCORD_TOKEN is read from configuration::client_tokens::DISCORD_TOKEN at compile time
# Make sure src/configuration.rs exists with your token before building

# Run the bot
CMD ["/app/bot"]

