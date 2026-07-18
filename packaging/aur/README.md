# AUR packaging (`optionfiles`)

Published at: https://aur.archlinux.org/packages/optionfiles

Every GitHub Release runs [Publish AUR](../../.github/workflows/publish-aur.yml), recalculates the tagged source SHA-256, commits `PKGBUILD` and `.SRCINFO` back to `main`, and publishes the package.

Required repository secret: `AUR_SSH_PRIVATE_KEY` containing the private key registered with the maintainer's AUR account.

Manual fallback:

```bash
./packaging/aur/publish.sh 0.2.0
```
