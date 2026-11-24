FROM scratch
ARG TARGETARCH
ARG VERSION
ARG REVISION
ARG SOURCE_URL
COPY ${TARGETARCH}/komodo-mcp /usr/local/bin/komodo-mcp
COPY ${TARGETARCH}/docs /usr/share/doc/komodo-mcp/
COPY ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
LABEL org.opencontainers.image.title="komodo-mcp" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.licenses="GPL-3.0-or-later" \
      org.opencontainers.image.source="https://github.com/enthouan/komodo-mcp" \
      org.opencontainers.image.description="Self-hosted Komodo MCP. Corresponding source: ${SOURCE_URL}"
USER 65532:65532
EXPOSE 3000
HEALTHCHECK --interval=15s --timeout=4s --start-period=10s --retries=3 CMD ["/usr/local/bin/komodo-mcp","--healthcheck"]
ENTRYPOINT ["/usr/local/bin/komodo-mcp"]
