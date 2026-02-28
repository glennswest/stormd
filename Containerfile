FROM scratch
COPY target/aarch64-unknown-linux-musl/release/stormd /stormd
VOLUME /var/log/stormd
EXPOSE 8080
ENTRYPOINT ["/stormd"]
