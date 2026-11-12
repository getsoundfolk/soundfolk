## Which file?

| System                                 | File                                     |
| -------------------------------------- | ---------------------------------------- |
| Debian 12+, Ubuntu 22.04+              | `Soundfolk_{{version}}_amd64.deb`        |
| Other Linux (glibc 2.35+)              | `Soundfolk_{{version}}_amd64.AppImage`   |
| Windows 10+                            | `Soundfolk_{{version}}_x64-setup.exe`    |
| macOS 10.13+ (Apple Silicon and Intel) | `Soundfolk_{{version}}_universal.dmg`    |

The other files (`.sig`, `.app.tar.gz`, `latest.json`) are used by the in-app updater.

## Known limitations

- The Windows installer isn't code-signed yet, and the macOS app isn't notarized yet, so the first
  launch needs one extra click ([how](https://github.com/getsoundfolk/soundfolk/blob/main/docs/install.md)).
