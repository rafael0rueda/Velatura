#!/bin/sh
# Install Fedora packages in CI, with a time limit on each try: a package
# mirror that stops answering fails the try within ten minutes, and the
# next one starts over with fresh metadata, instead of the job waiting for
# hours. A normal install takes about a minute.
#
#   scripts/ci-install.sh <package>...
for try in 1 2 3; do
    timeout 10m dnf install -y --setopt=install_weak_deps=False "$@" && exit 0
    echo "dnf did not finish (try $try of 3)" >&2
    dnf clean metadata >/dev/null 2>&1 || true
done
exit 1
