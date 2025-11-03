#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}" )/.." && pwd)"
OUT_DIR="${1:-${REPO_ROOT}/docs/src/cli}"

MAN_TMP="$(mktemp -d)"
trap 'rm -rf "${MAN_TMP}"' EXIT

cargo run --quiet --bin generate-manpage -- --dir "${MAN_TMP}" 1>&2

mkdir -p "${OUT_DIR}"
rm -f "${OUT_DIR}"/*.md
rm -f "${REPO_ROOT}/docs/src/cli-manpage.md"

declare -a sub_entries=()
top_file=""
top_title=""

for man_page in "${MAN_TMP}"/*.1; do
    [[ -f "${man_page}" ]] || continue
    base_name="$(basename "${man_page}" .1)"
    display="${base_name//-/ }"
    display="${display/conda dist/conda-dist}"

    out_file="${OUT_DIR}/${base_name}.md"
    {
        printf '# %s\n\n' "${display}"
        pandoc "${man_page}" -f man -t gfm --shift-heading-level-by=1
    } > "${out_file}"

    if [[ "${base_name}" == "conda-dist" ]]; then
        top_file="${base_name}.md"
        top_title="${display}"
    else
        sub_entries+=("${display}|${base_name}.md")
    fi
done

INDEX_FILE="${OUT_DIR}/index.md"
{
    printf '# CLI Reference\n\n'
    if [[ -n "${top_file}" ]]; then
        printf -- '- [%s](./%s)\n' "${top_title}" "${top_file}"
        if ((${#sub_entries[@]} > 0)); then
            sorted_entries="$(printf '%s\n' "${sub_entries[@]}" | sort)"
            while IFS='|' read -r title path; do
                [[ -n "${title}" && -n "${path}" ]] || continue
                printf '  - [%s](./%s)\n' "${title}" "${path}"
            done <<< "${sorted_entries}"
        fi
    else
        if ((${#sub_entries[@]} > 0)); then
            sorted_entries="$(printf '%s\n' "${sub_entries[@]}" | sort)"
            while IFS='|' read -r title path; do
                [[ -n "${title}" && -n "${path}" ]] || continue
                printf -- '- [%s](./%s)\n' "${title}" "${path}"
            done <<< "${sorted_entries}"
        fi
    fi
    printf '\n'
} > "${INDEX_FILE}"

echo "Manpage markdown written to ${OUT_DIR}" >&2
