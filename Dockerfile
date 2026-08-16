FROM debian:bookworm-slim

COPY dist/lql /usr/local/bin/lql
RUN chmod 0755 /usr/local/bin/lql

ENTRYPOINT ["/usr/local/bin/lql"]
CMD ["version"]
