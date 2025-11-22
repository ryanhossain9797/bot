# Cross-Platform Docker Builds (ARM → x86_64)

This guide explains how to build x86_64 (amd64) Docker images on an ARM Linux machine.

## Prerequisites

1. **Docker with buildx enabled** (usually included in Docker Desktop or Docker 19.03+)
2. **QEMU for emulation** (recommended for simplicity) or cross-compilation toolchain

## Setup

### 1. Enable Docker buildx

```bash
# Check if buildx is available
docker buildx version

# Create a multi-platform builder (if not exists)
docker buildx create --name multiarch --use

# Verify it's set up
docker buildx inspect --bootstrap
```

### 2. Install QEMU (for emulation - recommended)

QEMU allows Docker to emulate x86_64 on ARM, which is simpler than cross-compilation:

```bash
# On Debian/Ubuntu
sudo apt-get update
sudo apt-get install -y qemu-user-static binfmt-support

# Register QEMU with binfmt
sudo update-binfmts --enable qemu-x86_64
```

## Building x86_64 Images

### Option 1: Using Just (Recommended)

```bash
# Build and push x86_64 image
just build_push_amd64

# Or with a specific tag
just build_push_amd64 tag="v1.0.0"
```

### Option 2: Using Docker buildx directly

```bash
cd bot_hive

# Build for x86_64
docker buildx build \
    --platform linux/amd64 \
    -f Dockerfile \
    -t zireael9797/bot:latest \
    --push \
    ..
```

### Option 3: Build without pushing (for testing)

```bash
docker buildx build \
    --platform linux/amd64 \
    -f Dockerfile \
    -t zireael9797/bot:latest \
    --load \
    ..
```

**Note**: `--load` only works for single-platform builds. For multi-platform, use `--push` or `--output type=docker`.

### Option 4: Using docker-compose

Update `docker-compose.yml` to specify platform:

```yaml
services:
  bot:
    platform: linux/amd64
    build:
      context: ..
      dockerfile: bot_hive/Dockerfile
    # ... rest of config
```

Then build:
```bash
docker-compose build
```

## How It Works

1. **Build Stage**: 
   - Uses `--platform=$BUILDPLATFORM` to run on your native ARM architecture
   - Detects if cross-compilation is needed
   - If `TARGETPLATFORM != BUILDPLATFORM`, it:
     - Installs x86_64 cross-compilation toolchain
     - Compiles Rust code for x86_64 target
   - Copies the binary to a standard location

2. **Runtime Stage**:
   - Uses the base image for the target platform (x86_64)
   - Copies the cross-compiled binary

## Performance Notes

- **Cross-compilation**: Faster, but requires proper toolchain setup
- **QEMU emulation**: Slower (can be 5-10x), but simpler and more reliable
- For llama.cpp with native dependencies, QEMU emulation is often more reliable

## Troubleshooting

### Build fails with "exec format error"
- Install QEMU: `sudo apt-get install qemu-user-static`
- Ensure binfmt is enabled: `sudo update-binfmts --enable qemu-x86_64`

### Cross-compilation fails
- Try using QEMU emulation instead (remove cross-compilation logic)
- Or ensure all native dependencies (llama.cpp) can cross-compile

### "platform linux/amd64 not supported"
- Your base image (`zireael9797/bot-base:latest`) must also support x86_64
- Rebuild the base image for x86_64 or use a multi-arch base image

## Building Base Image for x86_64

If your base image doesn't support x86_64, build it:

```bash
cd bot_hive

docker buildx build \
    --platform linux/amd64 \
    -f Dockerfile.base \
    -t zireael9797/bot-base:latest \
    --push \
    .
```

## Multi-Architecture Builds

To build for both ARM and x86_64 simultaneously:

```bash
docker buildx build \
    --platform linux/amd64,linux/arm64 \
    -f Dockerfile \
    -t zireael9797/bot:latest \
    --push \
    ..
```

This creates a manifest that points to both architectures.

