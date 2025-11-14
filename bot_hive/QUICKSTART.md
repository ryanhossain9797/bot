# Quick Start Guide

## Prerequisites

1. **Docker** installed and running
2. **configuration.rs** file exists with your Discord token (already exists in your repo)

## Step 1: Build the Docker Image

From the `bot_hive` directory:

```bash
# Using docker build
docker build -f Dockerfile -t bot-hive:latest ..

# Or using docker-compose (recommended)
docker-compose build
```

**Note**: This will take a while (10-30 minutes) as it:
- Downloads Rust toolchain
- Compiles the entire project
- Includes the 8.4GB model file

You'll see output like:
```
[+] Building 0.1s (1/1) FINISHED
 => [internal] load build definition from Dockerfile
 => => transferring dockerfile: 2.00kB
...
```

## Step 2: Test Run (Foreground)

Run the container in the foreground to see logs:

```bash
docker run --rm --name bot-hive-test bot-hive:latest
```

This will:
- Start the bot
- Show all logs in your terminal
- Exit when you press Ctrl+C

You should see:
- Model loading messages
- LLM initialization
- Discord connection attempts
- Bot ready messages

## Step 3: Run in Background (Production-like)

Run as a detached container:

```bash
docker run -d \
  --name bot-hive \
  --restart unless-stopped \
  bot-hive:latest
```

## Step 4: Check Logs

View the logs:

```bash
docker logs bot-hive
```

Follow logs in real-time:

```bash
docker logs -f bot-hive
```

## Step 5: Stop the Container

```bash
docker stop bot-hive
```

Remove the container:

```bash
docker rm bot-hive
```

## Troubleshooting

### Check if container is running
```bash
docker ps -a | grep bot-hive
```

### Check container resource usage
```bash
docker stats bot-hive
```

### Interactive shell (for debugging)
```bash
docker exec -it bot-hive /bin/bash
```

### Verify files are present
```bash
docker exec bot-hive ls -lh /app/models/
docker exec bot-hive ls -lh /app/grammars/
```

### Common Issues

1. **Out of memory**: Make sure Docker has enough memory allocated (8GB+)
   - Docker Desktop: Settings → Resources → Memory
   - Increase to at least 8GB

2. **Model not found**: Check if model file exists:
   ```bash
   ls -lh models/Qwen2.5-14B-Instruct-Q4_K_M.gguf
   ```

3. **Build fails**: Make sure configuration.rs exists:
   ```bash
   ls -la src/configuration.rs
   ```

4. **Discord connection fails**: Check your token in configuration.rs

## Using Docker Compose

Alternatively, use docker-compose:

```bash
# Build and run
docker-compose up -d

# View logs
docker-compose logs -f

# Stop
docker-compose down
```

