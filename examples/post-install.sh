#!/bin/sh
set -eu

prefix="${CONDA_DIST_INSTALL_PREFIX:-}"
project="${CONDA_DIST_PROJECT_NAME:-the bundle}"

cat <<EOF
Post-install steps for ${project} complete.

Installation prefix: ${prefix}

You can launch the bundled environment by running:
  ${prefix}/bin/bash
EOF

exit 0
