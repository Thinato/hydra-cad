# docker build -t step2gltf tools/step2gltf
# docker run --rm -v "$PWD:/w" -w /w step2gltf in.step out.glb
FROM debian:bookworm-slim AS build
# Debian's OCCT CMake targets also link TBB and fontconfig, whose -dev packages it does not pull in.
RUN apt-get update && apt-get install -y --no-install-recommends \
      cmake g++ make libocct-data-exchange-dev libtbb-dev libfontconfig-dev \
    && rm -rf /var/lib/apt/lists/*
COPY CMakeLists.txt step2gltf.cpp make_fixtures.cpp /src/
RUN cmake -S /src -B /build -DCMAKE_BUILD_TYPE=Release && cmake --build /build -j"$(nproc)" \
    && cmake --install /build --prefix /usr/local --strip

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends libocct-data-exchange-7.6 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/step2gltf /usr/local/bin/make_fixtures /usr/local/bin/
ENTRYPOINT ["step2gltf"]
