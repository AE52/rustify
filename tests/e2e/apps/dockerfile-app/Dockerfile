# Static "hello" served by nginx, with a container HEALTHCHECK so the E2E
# harness can exercise the Dockerfile build pack + health-check wiring.
FROM nginx:alpine

COPY index.html /usr/share/nginx/html/index.html

EXPOSE 80

HEALTHCHECK --interval=5s --timeout=3s --start-period=5s --retries=5 \
  CMD wget -q -O /dev/null http://localhost:80/ || exit 1
