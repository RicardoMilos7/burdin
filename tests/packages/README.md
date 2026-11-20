# Official-package fixtures

Stand-ins for the real official packages (`packages/`), so `tests/check_packages.py` does not depend on them: it points
`BURDIN_PACKAGES` here (spec/packages.md, PKG16). `shout` depends on `mark`, both with `name = true`.
