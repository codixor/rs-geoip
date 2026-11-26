#!/bin/sh
# Run with rs-geo-country-build on PATH or COUNTRY_DB_BUILDER set to its path.
set -eu
updater=$(dirname "$0")/../scripts/update-country-db.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' 0
trap 'exit 1' HUP INT TERM

# Simulate a successful download of each local response body, including HTTP 204.
cat > "$work/curl" <<'SH'
#!/bin/sh
set -eu
while [ "$1" != --output ]; do shift; done
cp "$4" "$2"
SH
chmod +x "$work/curl"
export PATH="$work:$PATH"

v4=$work/ipv4
v6=$work/ipv6
db="$work/installed country.bin"
printf '10,20,AE\n' > "$v4"
printf '15,30,US\n' > "$v6"
sh "$updater" "$v4" "$v6" "$db" > "$work/log"
"${COUNTRY_DB_BUILDER:-rs-geo-country-build}" check "$db" > "$work/info"
grep -q 'IPv4 ranges: 1' "$work/info"
grep -q 'IPv6 ranges: 1' "$work/info"
cp "$db" "$work/original"

for family in IPv4 IPv6; do
    case $family in IPv4) feed=$v4 ;; IPv6) feed=$v6 ;; esac
    cp "$feed" "$work/good"
    for body in empty whitespace; do
        case $body in
            empty) : > "$feed" ;;
            whitespace) printf '  \r\n\t\r\n' > "$feed" ;;
        esac
        if sh "$updater" "$v4" "$v6" "$db" > "$work/log" 2>&1; then
            echo "accepted $body $family download" >&2
            exit 1
        fi
        grep -q "$family download contains no data rows" "$work/log"
        cmp "$db" "$work/original"
    done
    cp "$work/good" "$feed"
done
echo 'Normal update passed. All four empty-feed cases preserved the installed database.'
