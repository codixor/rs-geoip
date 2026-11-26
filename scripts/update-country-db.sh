#!/bin/sh
# Usage: update-country-db.sh [IPV4_URL IPV6_URL [OUTPUT]]
set -eu

case $# in
    0) ;;
    2|3) IPV4_URL=$1; IPV6_URL=$2 ;;
    *) echo 'usage: update-country-db.sh [IPV4_URL IPV6_URL [OUTPUT]]' >&2; exit 2 ;;
esac
: "${IPV4_URL:?set IPV4_URL or pass both CSV URLs}"
: "${IPV6_URL:?set IPV6_URL or pass both CSV URLs}"
output=${3:-${COUNTRY_DB_OUT:-country.bin}}
builder=${COUNTRY_DB_BUILDER:-rs-geo-country-build}
label=${COUNTRY_DB_LABEL:-user-country PDDL-1.0}

# Keep utility operands separate from options, including relative paths starting with '-'.
case $output in /*) ;; *) output=./$output ;; esac
if [ -d "$output" ]; then
    echo "output is a directory: $output" >&2
    exit 1
fi
stage=$(mktemp -d "$(dirname "$output")/.rs-geo-country.XXXXXXXX")
trap 'rm -rf "$stage"' 0
trap 'exit 1' HUP INT TERM

v4=$stage/ipv4.csv
v6=$stage/ipv6.csv
db=$stage/country.bin
download_feed() {
    curl --fail --silent --show-error --location --output "$3" -- "$2"
    if ! LC_ALL=C grep -q '[^[:space:]]' "$3"; then
        echo "$1 download contains no data rows" >&2
        exit 1
    fi
}
download_feed IPv4 "$IPV4_URL" "$v4"
download_feed IPv6 "$IPV6_URL" "$v6"
"$builder" --ipv4 "$v4" --ipv6 "$v6" --label "$label" --out "$db"
"$builder" check "$db"
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$stage" && sha256sum ipv4.csv ipv6.csv country.bin)
else
    (cd "$stage" && shasum -a 256 ipv4.csv ipv6.csv country.bin)
fi
mv -f "$db" "$output"
