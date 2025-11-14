# Docker Setup for Bot Hive

## Building the Docker Image

**Prerequisites**: You must have a `configuration.rs` file with your Discord token before building.

1. Copy the template and set your token:
   ```bash
   cp src/configuration.rs.template src/configuration.rs
   # Edit src/configuration.rs and set your DISCORD_TOKEN
   ```

2. Build the image (from bot_hive directory):
   ```bash
   docker build -f Dockerfile -t bot-hive:latest ..
   ```
   
   Or use docker-compose:
   ```bash
   docker-compose build
   ```

**Note**: The build will include the 8.4GB model file, so it may take some time and require significant disk space.

## Running with Docker

### Basic Run

```bash
docker run -d \
  --name bot-hive \
  bot-hive:latest
```

**Note**: The Discord token is compiled into the binary from `configuration.rs`, so no environment variable is needed at runtime.

### Using Docker Compose

Run with docker-compose:
```bash
docker-compose up -d
```

**Note**: Make sure `configuration.rs` with your Discord token exists before building the image.

## Environment Variables

- **MODEL_PATH** (optional): Path to the model file (default: `/app/models/Qwen2.5-14B-Instruct-Q4_K_M.gguf`)
- **RUST_LOG** (optional): Logging level (default: `info`)

**Note**: `DISCORD_TOKEN` is currently read from `configuration.rs` at compile time, not from environment variables.

## Configuration

**Important**: The bot reads the Discord token from `configuration.rs` at compile time. You **must** provide this file before building the Docker image.

1. Copy the template and set your token:
   ```bash
   cp bot_hive/src/configuration.rs.template bot_hive/src/configuration.rs
   # Edit bot_hive/src/configuration.rs and set your DISCORD_TOKEN
   ```

2. The `configuration.rs` file will be compiled into the binary during the Docker build.

**Note**: For production deployments, consider modifying the code to read `DISCORD_TOKEN` from environment variables instead of compile-time constants.

## Resource Requirements

The bot requires significant memory for the LLM model:

- **Minimum**: 8GB RAM
- **Recommended**: 16GB+ RAM
- **Model size**: ~8.37 GB (Qwen2.5-14B-Instruct-Q4_K_M.gguf)

Adjust memory limits in `docker-compose.yml` or use `--memory` flag with `docker run`.

## Included Files

The Docker image includes:
- Compiled binary (`/app/bot`)
- Model file (`/app/models/Qwen2.5-14B-Instruct-Q4_K_M.gguf`)
- Grammar file (`/app/bot_hive/grammars/response.gbnf`) - embedded at compile time

## Troubleshooting

### Check logs
```bash
docker logs bot-hive
```

### Interactive shell
```bash
docker exec -it bot-hive /bin/bash
```

### Verify model file
```bash
docker exec bot-hive ls -lh /app/models/
```

## Build Context

**Important**: The Dockerfile is in `bot_hive/` but the build context is the parent directory (to access `lib_hive` framework). When building:

- **With docker build**: Use `-f Dockerfile` and `..` as context:
  ```bash
  docker build -f Dockerfile -t bot-hive:latest ..
  ```

- **With docker-compose**: The context is automatically set to parent directory

