FROM searxng/searxng:latest

COPY settings.yml /etc/searxng/settings.yml

RUN SECRET=$(head -c 32 /dev/urandom | base64 | tr -dc 'a-zA-Z0-9') && \
    sed -i "s|__SECRET__|$SECRET|" /etc/searxng/settings.yml

HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD wget -q -O /dev/null http://localhost:8080/ || exit 1
