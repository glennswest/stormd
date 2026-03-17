FROM scratch
COPY target/aarch64-unknown-linux-musl/release/stormd /stormd
COPY target/aarch64-unknown-linux-musl/release/stormsh /stormsh
VOLUME /data/minio
VOLUME /var/log/stormd
EXPOSE 9080 9000 22
ENTRYPOINT ["/stormd"]
