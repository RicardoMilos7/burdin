# Official packages

The official packages ship with the compiler. A program reaches one with `use <name>`, and no `burdin.toml` entry is needed. A build records each official package in `burdin.lock` with `source = "official"` and a `checksum` of its files (`spec/packages.md`).

| Package | Version | Official pin | Requirements | Page |
| --- | --- | --- | --- | --- |
| `crypto` | `0.1.0` | `sha256:bdcac6d2320a9856457724e6cc0138812d16ee5865463dd42b85c66a301f2347` | Nothing beyond the compiler | [crypto](crypto.md) |
| `csv` | `0.1.0` | `sha256:19739c027c548a1551329cbea1f55ff5456e5ae256567350854db2f13de7a19b` | Nothing beyond the compiler | [csv](csv.md) |
| `http` | `0.1.0` | `sha256:8a6cb42529db52caec916e01b3bc787f925427eb27b9973121ea3a0654e9bd7d` | Nothing on Windows; on Linux and macOS, OpenSSL 3 through `tls` | [http](http.md) |
| `json` | `0.1.0` | `sha256:0f4a42c988397521b80ded5df19343d1692a686cf1101cd366fef6311c22a1e8` | Nothing beyond the compiler | [json](json.md) |
| `math` | `0.1.0` | `sha256:d6dee722fe0e6afa82b4cdf2e5d7187d493dbe62895fc39e783ad13de012415f` | Nothing beyond the compiler | [math](math.md) |
| `regex` | `0.1.0` | `sha256:7d687124ec676c7c263b325c83365de7128095cf31866a441eb2283c4f1bae43` | Nothing beyond the compiler | [regex](regex.md) |
| `tls` | `0.1.0` | `sha256:75b581628ef1cca7265b06a021bcc55f79166dc994a2c57e274b6d26e97947df` | Linux and macOS: OpenSSL 3; Windows: nothing | [tls](tls.md) |
| `toml` | `0.1.0` | `sha256:38dd59b6f0ae98350beafbfbd16218baa0515c3de853312b74ff9c9ae98a44a9` | Nothing beyond the compiler | [toml](toml.md) |
| `xml` | `0.1.0` | `sha256:4c15016426a3b9e2c3643bca08a4112cdc87812d85428bb86cdc2330f13160e6` | Nothing beyond the compiler | [xml](xml.md) |
| `yaml` | `0.1.0` | `sha256:65cab4ece8cade4b73b18df743d1210b965932926bbedbe9be15928fddb12f24` | Nothing beyond the compiler | [yaml](yaml.md) |

The pins are the checksums that the lockfile recorded for the packages of this checkout. They change whenever a package's files change, so a checkout that differs from this one has different pins; the `burdin.lock` a build writes is the authority.
