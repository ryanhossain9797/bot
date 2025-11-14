# Build stage
FROM rustlang/rust:nightly-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libclang-dev \
    cmake \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy workspace files (build context should be parent directory)
COPY Cargo.toml Cargo.lock ./
COPY lib_hive ./lib_hive
COPY bot_hive ./bot_hive

# Build the project in release mode
RUN cargo build --release --package bot

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libgomp1 \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/bot /app/bot

# Copy model file
COPY --from=builder /app/bot_hive/models/Qwen2.5-14B-Instruct-Q4_K_M.gguf /app/models/Qwen2.5-14B-Instruct-Q4_K_M.gguf

# Copy grammar file (included at compile time, but kept for reference)
COPY --from=builder /app/bot_hive/grammars/response.gbnf /app/grammars/response.gbnf

# Set environment variables
ENV MODEL_PATH=/app/models/Qwen2.5-14B-Instruct-Q4_K_M.gguf
ENV RUST_LOG=info

# Note: configuration.rs must be provided at build time
# The DISCORD_TOKEN is read from configuration::client_tokens::DISCORD_TOKEN at compile time
# Make sure src/configuration.rs exists with your token before building

# Run the bot
CMD ["/app/bot"]

