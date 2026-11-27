#!/usr/bin/env bash
# Checks that every Slint `Text` elides instead of widening its layout. A `Text` without `overflow: elide` reports
# its whole text (or its longest word when wrapping) as min-width, so a long value pushes the layout past its area.
# Fails for every `Text { ... }` block (also `inherits Text { ... }`) in crates/ui/ui/**/*.slint that contains neither
# `overflow: elide` nor an exception comment `// no-elide: <reason>`. Prints `file:line` per finding.
# Usage (repo root): scripts/check-text-overflow.sh
set -euo pipefail

cd "$(dirname "$0")/.." || exit 1

mapfile -t slint_files < <(find crates/ui/ui -name '*.slint' | sort)

awk '
    function report() {
        if (!elides) {
            printf "%s:%d: Text without overflow: elide (add it, or a // no-elide: <reason> comment)\n", start_file, start_line
            findings++
        }
        in_block = 0
    }
    FNR == 1 && in_block { report() }
    {
        line = $0
        if (!in_block && match(line, /(^|[^A-Za-z0-9_-])Text[ \t]*\{/)) {
            in_block = 1
            depth = 0
            elides = 0
            start_file = FILENAME
            start_line = FNR
            line = substr(line, RSTART)
        }
        if (!in_block) next
        if (line ~ /overflow[ \t]*:[ \t]*elide/ || line ~ /\/\/[ \t]*no-elide:/) elides = 1
        code = line
        sub(/\/\/.*/, "", code)
        opened = gsub(/\{/, "{", code)
        closed = gsub(/\}/, "}", code)
        depth += opened - closed
        if (depth <= 0) report()
    }
    END {
        if (in_block) report()
        if (findings) {
            printf "%d Text block(s) without overflow: elide\n", findings
            exit 1
        }
    }
' "${slint_files[@]}"
