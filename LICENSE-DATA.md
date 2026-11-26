# Dataset licences

The repository and crates.io package ship no databases or source datasets. MIT and Apache-2.0 cover the crate's code. Data you fetch or build has separate terms.

## Country

The recommended country table uses the two numeric CSVs from `sapics/ip-location-db`'s `user-country` dataset. On 26 November 2026, the publisher listed the compiled dataset under the Open Data Commons Public Domain Dedication and License 1.0 and described its focus as the end user's country. The publisher identifies regional registry statistics, routing archives and operator geofeeds among its inputs. [Publisher's dataset description](https://github.com/sapics/ip-location-db#original-databases-update-daily) · [PDDL 1.0](https://opendatacommons.org/licenses/pddl/1-0/)

The builder also accepts other sources with the same CSV layout. Set a label identifying the dataset you used. Each source's licence applies to data built from it.

## City

The city tools can build local packs from GeoLite2-derived city CSVs. Anyone who builds, embeds or distributes such a pack is responsible for complying with the applicable GeoLite2 terms and the licences of any other inputs they use. [MaxMind GeoLite2 licence](https://www.maxmind.com/en/geolite2/eula) · [Upstream city sources](https://github.com/sapics/ip-location-db#city)
