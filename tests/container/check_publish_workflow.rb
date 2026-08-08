#!/usr/bin/env ruby

require "yaml"

workflow = YAML.load_file(ARGV.fetch(0))
triggers = workflow["on"] || workflow[true]

def assert_equal(expected, actual, label)
  return if expected == actual

  raise "#{label}: expected #{expected.inspect}, got #{actual.inspect}"
end

assert_equal(
  {
    "push" => {"tags" => ["v[0-9]+.[0-9]+.[0-9]+"]},
    "repository_dispatch" => {"types" => ["publish-release"]}
  },
  triggers,
  "release triggers"
)
assert_equal(
  {
    "RELEASE_TAG" => "${{ github.event.client_payload.release_tag || github.ref_name }}",
    "EXPECTED_RELEASE_SHA" => "${{ github.event_name == 'push' && github.event.after || '' }}"
  },
  workflow["env"],
  "release environment"
)
assert_equal(nil, workflow["permissions"], "global permissions")
assert_equal(["gate", "publish"], workflow.fetch("jobs").keys, "release jobs")

gate = workflow.fetch("jobs").fetch("gate")
assert_equal(
  ["runs-on", "permissions", "outputs", "steps"].sort,
  gate.keys.sort,
  "gate keys"
)
assert_equal("ubuntu-latest", gate["runs-on"], "gate runner")
assert_equal({"contents" => "read"}, gate["permissions"], "gate permissions")
assert_equal(
  {"release_sha" => "${{ steps.release.outputs.sha }}"},
  gate["outputs"],
  "gate outputs"
)

gate_steps = gate.fetch("steps")
assert_equal(7, gate_steps.length, "gate step count")
assert_equal(
  {
    "uses" => "actions/checkout@v4",
    "with" => {
      "ref" => "refs/tags/${{ env.RELEASE_TAG }}",
      "persist-credentials" => false
    }
  },
  gate_steps[0],
  "gate checkout"
)
assert_equal(
  {
    "name" => "Verify immutable release tag",
    "id" => "release",
    "run" => <<~'SHELL'
      release_tag_pattern='^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
      [[ "$RELEASE_TAG" =~ $release_tag_pattern ]]
      release_sha=$(git rev-parse "$RELEASE_TAG^{commit}")
      test "$release_sha" = "$(git rev-parse HEAD)"
      if [[ "$GITHUB_EVENT_NAME" == "push" ]]; then
        test "$release_sha" = "$EXPECTED_RELEASE_SHA"
      fi
      echo "sha=$release_sha" >> "$GITHUB_OUTPUT"
    SHELL
  },
  gate_steps[1],
  "release tag verification"
)
assert_equal(
  {
    "uses" => "dtolnay/rust-toolchain@stable",
    "with" => {"toolchain" => "1.97.1"}
  },
  gate_steps[2],
  "gate Rust toolchain"
)
assert_equal("Verify pinned Orca source", gate_steps[3]["name"], "source gate")
assert_equal(
  {"uses" => "docker/setup-buildx-action@v3"},
  gate_steps[4],
  "gate Buildx setup"
)
assert_equal(
  {
    "uses" => "docker/build-push-action@v6",
    "with" => {
      "context" => ".",
      "platforms" => "linux/amd64",
      "load" => true,
      "provenance" => false,
      "tags" => "orca-slicer-api:release-gate"
    }
  },
  gate_steps[5],
  "gate image build"
)
assert_equal(
  {
    "run" => "python3 tests/container/test_runtime.py orca-slicer-api:release-gate\n",
    "env" => {"CONTAINER_ENGINE" => "docker"}
  },
  gate_steps[6],
  "restricted runtime gate"
)

publish = workflow.fetch("jobs").fetch("publish")
assert_equal(
  ["needs", "runs-on", "permissions", "steps"].sort,
  publish.keys.sort,
  "publish keys"
)
assert_equal("gate", publish["needs"], "publish dependency")
assert_equal("ubuntu-latest", publish["runs-on"], "publish runner")
assert_equal(
  {
    "contents" => "read",
    "packages" => "write",
    "id-token" => "write",
    "attestations" => "write"
  },
  publish["permissions"],
  "publish permissions"
)
assert_equal(
  [
    {
      "uses" => "actions/checkout@v4",
      "with" => {
        "ref" => "${{ needs.gate.outputs.release_sha }}",
        "persist-credentials" => false
      }
    },
    {
      "uses" => "docker/setup-buildx-action@v3",
      "id" => "buildx",
      "with" => {"driver" => "docker-container"}
    },
    {
      "uses" => "docker/login-action@v3",
      "with" => {
        "registry" => "ghcr.io",
        "username" => "${{ github.actor }}",
        "password" => "${{ secrets.GITHUB_TOKEN }}"
      }
    },
    {
      "uses" => "docker/metadata-action@v5",
      "id" => "meta",
      "with" => {
        "images" => "ghcr.io/${{ github.repository }}",
        "tags" => "type=semver,pattern={{version}},value=${{ env.RELEASE_TAG }}\n",
        "flavor" => "latest=false",
        "labels" => "org.opencontainers.image.revision=${{ needs.gate.outputs.release_sha }}\n"
      }
    },
    {
      "uses" => "docker/build-push-action@v6",
      "id" => "build",
      "with" => {
        "builder" => "${{ steps.buildx.outputs.name }}",
        "context" => ".",
        "platforms" => "linux/amd64",
        "push" => true,
        "tags" => "${{ steps.meta.outputs.tags }}",
        "labels" => "${{ steps.meta.outputs.labels }}",
        "provenance" => "mode=max",
        "sbom" => true
      }
    },
    {
      "uses" => "actions/attest-build-provenance@v2",
      "with" => {
        "subject-name" => "ghcr.io/${{ github.repository }}",
        "subject-digest" => "${{ steps.build.outputs.digest }}",
        "push-to-registry" => true
      }
    }
  ],
  publish.fetch("steps"),
  "publish steps"
)
