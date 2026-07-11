#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
SDK=~/rm-sdk-3.26
ENV=$(ls "$SDK"/environment-setup-* | head -n1)
unset LD_LIBRARY_PATH
source "$ENV"
[ -f ../quill/build/libquill.so ] || (cd ../quill && ./build.sh)
cat > /tmp/inktype-sdk-cc.sh <<EOF
#!/bin/bash
exec $CC "\$@"
EOF
chmod +x /tmp/inktype-sdk-cc.sh
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=/tmp/inktype-sdk-cc.sh
cargo build --release --target aarch64-unknown-linux-gnu --features takeover "$@"
echo "built: target/aarch64-unknown-linux-gnu/release/inktype"
