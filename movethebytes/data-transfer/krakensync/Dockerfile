FROM rust:1.63-bullseye as builder
WORKDIR /usr/src/testplan

RUN apt-get update && apt-get install -y cmake protobuf-compiler

ARG PLAN_PATH="./"
COPY ./plan/${PLAN_PATH} ./plan
RUN mv ./plan/fixtures /usr/src/fixtures

RUN cd ./plan/ \
    && cargo build \
    && mv /usr/src/testplan/plan/target/debug/krakensync /usr/local/bin/testplan

FROM debian:bullseye-slim
COPY --from=builder /usr/local/bin/testplan /usr/local/bin/testplan
COPY --from=builder /usr/src/fixtures /fixtures
EXPOSE 6060
ENV RUST_BACKTRACE=1
ENTRYPOINT ["testplan"]
