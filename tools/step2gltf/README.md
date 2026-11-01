# step2gltf

Converts a STEP model into a binary glTF (`.glb`) that Hydra can load. Part names and colours
are kept. It uses OpenCASCADE (OCCT) 7.6 or newer.

## Build

- **Linux (Debian/Ubuntu):** `sudo apt install cmake g++ make libocct-data-exchange-dev libtbb-dev libfontconfig-dev`
- **macOS:** `brew install opencascade cmake`
- **Windows (untested):** `vcpkg install opencascade`, then add `-DCMAKE_TOOLCHAIN_FILE=<vcpkg>/scripts/buildsystems/vcpkg.cmake`

Then, from the repo root (`target/` is git-ignored):
`cmake -S tools/step2gltf -B target/step2gltf && cmake --build target/step2gltf`.

**Docker** (no OCCT needed on the host), from the repository root:

    docker build -t step2gltf tools/step2gltf
    docker run --rm -v "$PWD:/w" -w /w step2gltf in.step out.glb

## Usage

    step2gltf <in.step> <out.glb> [--linear MM] [--angular DEG]

`--linear` (default 0.1 mm) is how far the mesh may stray from the true surface; `--angular`
(default 20 degrees) limits the angle between neighbouring facets. Coarser values (for example
`--linear 0.5`) give smaller files. On success it prints one summary line; on failure it prints
one line on stderr and exits non-zero.

## Coordinates and units

Input is CAD convention: Z up, in the STEP file's length unit (normally mm). Output is standard
glTF: Y up, metres. A CAD point (x, y, z) in mm becomes glTF (x/1000, z/1000, -y/1000). Nothing else
is moved, so model your part where Hydra expects it:

- **Valves:** the origin is the mounting-pattern datum, the mounting face lies in z = 0 and the
  body extends into +z.
- **Blocks:** model in block coordinates; the mount faces are described in the catalogue.

## Fixtures

`fixtures/models/` holds demo models made by `make_fixtures`. To regenerate them (from the repo root):

    docker run --rm -v "$PWD:/w" -w /w --entrypoint make_fixtures step2gltf fixtures/models
    docker run --rm -v "$PWD:/w" -w /w step2gltf fixtures/models/demo-valve-cetop3.step fixtures/models/demo-valve-cetop3.glb
    docker run --rm -v "$PWD:/w" -w /w step2gltf fixtures/models/demo-block.step fixtures/models/demo-block.glb
    docker run --rm -v "$PWD:/w" -w /w step2gltf fixtures/models/demo-cartridge.step fixtures/models/demo-cartridge.glb
