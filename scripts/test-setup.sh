#!/bin/sh
# Offline setup-hook regression: mock every download and use an isolated plugin copy.
set -eu

source_root="$(cd "$(dirname "$0")/.." && pwd)"
test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT
mkdir -p "$test_root/path"
for cmd in dirname pwd sed head mktemp mkdir rm awk chmod mv; do
  ln -s "$(command -v "$cmd")" "$test_root/path/$cmd"
done
cat > "$test_root/path/uname" <<'MOCK'
#!/bin/sh
case "$1" in -s) printf 'Linux\n';; -m) printf 'x86_64\n';; esac
MOCK
cat > "$test_root/path/curl" <<'MOCK'
#!/bin/sh
url=
dest=
while [ "$#" -gt 0 ]; do
  case "$1" in
    http*) url="$1" ;;
    -o) shift; dest="$1" ;;
  esac
  shift
done
case "$url" in
  */SHA256SUMS)
    [ "$SC_MOCK_SUMS" != missing ] || exit 22
    printf '%s\n' "$SC_MOCK_SUMS" > "$dest" ;;
  *) printf abc > "$dest" ;;
esac
MOCK
chmod +x "$test_root/path/uname" "$test_root/path/curl"

hash='ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
asset='semantic-compact-x86_64-unknown-linux-gnu'
run_case() {
  name="$1"
  sums="$2"
  has_hash="$3"
  expect_install="$4"
  plugin="$test_root/$name"
  mkdir -p "$plugin/scripts" "$plugin/.claude-plugin"
  cp "$source_root/scripts/setup.sh" "$plugin/scripts/setup.sh"
  printf '{"version":"0.2.7"}\n' > "$plugin/.claude-plugin/plugin.json"
  rm -f "$test_root/path/sha256sum" "$test_root/path/shasum"
  if [ "$has_hash" = yes ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      ln -s "$(command -v sha256sum)" "$test_root/path/sha256sum"
    else
      ln -s "$(command -v shasum)" "$test_root/path/shasum"
    fi
  else
    rm -f "$test_root/path/sha256sum" "$test_root/path/shasum"
  fi
  SC_MOCK_SUMS="$sums" SC_RELEASES_URL='https://invalid.example/releases' \
    PATH="$test_root/path" /bin/sh "$plugin/scripts/setup.sh" > "$test_root/$name.log" 2>&1 || {
      printf '%s: setup returned nonzero\n' "$name" >&2; exit 1;
    }
  if [ "$expect_install" = yes ]; then
    [ -x "$plugin/bin/semantic-compact" ] || { printf '%s: no install\n' "$name" >&2; exit 1; }
  else
    [ ! -e "$plugin/bin/semantic-compact" ] || { printf '%s: unverified install\n' "$name" >&2; exit 1; }
  fi
}
run_case valid "$hash  $asset" yes yes
run_case mismatch "0000000000000000000000000000000000000000000000000000000000000000  $asset" yes no
run_case missing_list missing yes no
run_case missing_entry "$hash  other-asset" yes no
run_case no_hash "$hash  $asset" no no
printf 'setup checksum cases passed\n'
