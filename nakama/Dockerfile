# Builds the Zanshin runtime module as a Go plugin and bundles it into a Nakama image.
# The plugin must be compiled with the exact Go toolchain of the target Nakama release:
# keep the two image tags in sync (see https://heroiclabs.com/docs/nakama/server-framework/go-runtime/).
ARG NAKAMA_VERSION=3.22.0
FROM heroiclabs/nakama-pluginbuilder:${NAKAMA_VERSION} AS builder
ENV GO111MODULE=on CGO_ENABLED=1
WORKDIR /backend
COPY sdk/go /sdk/go
COPY nakama/go.mod nakama/go.sum* ./
RUN sed -i 's#=> ../sdk/go#=> /sdk/go#' go.mod && go mod download
COPY nakama/ .
RUN go build --trimpath --buildmode=plugin -o ./zanshin.so

FROM heroiclabs/nakama:${NAKAMA_VERSION}
ARG NAKAMA_VERSION
ARG VERSION=dev
ARG COMMIT=unknown
LABEL org.opencontainers.image.title="zanshin-nakama" \
      org.opencontainers.image.description="Nakama ${NAKAMA_VERSION} with the Zanshin runtime module" \
      org.opencontainers.image.source="https://github.com/rep0rtDev/zanshin" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${COMMIT}"
COPY --from=builder /backend/zanshin.so /nakama/data/modules/
