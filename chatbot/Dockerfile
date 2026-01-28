FROM rustlang/rust:nightly AS builder

RUN apt-get update

# Install build dependencies
RUN apt-get install -y \
    pkg-config \
    libssl-dev \
    libclang-dev \
    cmake \
    build-essential \
    git \
    libvulkan-dev \
    glslc \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-unknown-linux-gnu

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY framework ./framework
COPY chatbot/src ./chatbot/src
COPY chatbot/Cargo.toml ./chatbot/
COPY chatbot/grammars ./chatbot/grammars
COPY .git ./.git

RUN cargo build --release --package chatbot

# Capture git commit message
RUN git log -1 --pretty=%B > /app/commit.txt || echo "unknown" > /app/commit.txt

FROM zireael9797/bot-base:latest


COPY --from=builder /app/target/release/chatbot /app/chatbot

# Copy commit message file
COPY --from=builder /app/commit.txt /app/commit.txt

# Note: configuration.rs must be provided at build time
# The DISCORD_TOKEN is read from configuration::client_tokens::DISCORD_TOKEN at compile time
# Make sure src/configuration.rs exists with your token before building

# Run the bot
CMD ["/app/chatbot"]