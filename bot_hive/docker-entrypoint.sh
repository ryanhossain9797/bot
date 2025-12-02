#!/bin/bash
set -e

echo "Starting Ollama service with ROCm..."
# Start Ollama in the background (already configured for ROCm in the base image)
ollama serve &

# Store the PID
OLLAMA_PID=$!

# Wait for Ollama to be ready
echo "Waiting for Ollama to be ready..."
max_attempts=30
attempt=0
while ! curl -s http://localhost:11434/api/tags > /dev/null 2>&1; do
    attempt=$((attempt + 1))
    if [ $attempt -ge $max_attempts ]; then
        echo "Failed to connect to Ollama after $max_attempts attempts"
        exit 1
    fi
    echo "Attempt $attempt/$max_attempts - waiting for Ollama..."
    sleep 2
done

echo "Ollama is ready!"

# Verify model is available
MODEL_NAME="qwen2.5:14b"
echo "Verifying model $MODEL_NAME is available..."
if curl -s http://localhost:11434/api/tags | grep -q "$MODEL_NAME"; then
    echo "Model $MODEL_NAME is ready."
else
    echo "WARNING: Model $MODEL_NAME not found, but continuing anyway..."
fi

# Run the bot application
echo "Starting the bot application..."
exec /app/bot

# If the app exits, cleanup
kill $OLLAMA_PID 2>/dev/null || true

