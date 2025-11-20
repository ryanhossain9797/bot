# Build stage
FROM rustlang/rust:nightly-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libclang-dev \
    cmake \
    build-essential \
    git \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy workspace files (build context should be parent directory)
COPY Cargo.toml Cargo.lock ./
COPY lib_hive ./lib_hive
COPY bot_hive ./bot_hive
COPY .git ./.git

# Build the project in release mode
RUN cargo build --release --package bot

# Capture git commit message
RUN git log -1 --pretty=%B > /app/commit.txt || echo "unknown" > /app/commit.txt

# Runtime stage - uses base image from Docker Hub
# Replace 'zireael9797' with your actual Docker Hub username
FROM zireael9797/bot-base:latest

# Copy binary from builder (only this changes on code updates)
COPY --from=builder /app/target/release/bot /app/bot

# Copy commit message file
COPY --from=builder /app/commit.txt /app/commit.txt

# Note: configuration.rs must be provided at build time
# The DISCORD_TOKEN is read from configuration::client_tokens::DISCORD_TOKEN at compile time
# Make sure src/configuration.rs exists with your token before building

# Run the bot
CMD ["/app/bot"]

