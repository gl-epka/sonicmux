# Third-party licenses

SonicMux is licensed under `MIT OR Apache-2.0`. Its GUI preview bundles also
contain separate FFmpeg and FFprobe executable programs.

## FFmpeg 8.1.2

- Project: <https://ffmpeg.org/>
- Source: <https://ffmpeg.org/releases/ffmpeg-8.1.2.tar.xz>
- License: GNU Lesser General Public License, version 2.1 or later
- Copyright: the FFmpeg developers and contributors

The bundled programs are built without `--enable-gpl` and without
`--enable-nonfree`. They are invoked as external child processes and are not
linked into SonicMux.

Every GitHub Release that contains FFmpeg binaries also contains the exact
official source archive, detached signature, public release-signing key, build
configuration and hashes, `changes.diff`, `COPYING.LGPLv2.1`, and this notice.
The source and build instructions can therefore be obtained from the same
server and release page as the executable bundle.

FFmpeg is a trademark of Fabrice Bellard, originator of the FFmpeg project.
SonicMux is not affiliated with or endorsed by the FFmpeg project.
