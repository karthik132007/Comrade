# Native voice components and source access

Comrade's own source retains its existing MIT license in `COMRADE-LICENSE`
(the repository's root `LICENSE`). The native voice libraries include code
under additional licenses listed below. Their license texts accompany this
file. This notice does not replace those terms or change the author's license
for Comrade's original code.

## eSpeak NG 1.52.0.1

The sherpa-onnx 1.12.9 native voice library contains the eSpeak NG fork from
`csukuangfj/espeak-ng`, commit
`f6fed6c58b5e0998b8e68c6610125e2d07d595a7`. Its upstream README identifies
the license as **GNU GPL version 3 or later**. Source headers include
Copyright (C) 2005 to 2014 Jonathan Duddington and Copyright (C) 2015-2017
Reece H. Dunn; the source retains the component's complete per-file notices.

- [Exact source archive](https://github.com/csukuangfj/espeak-ng/archive/f6fed6c58b5e0998b8e68c6610125e2d07d595a7.zip)
- Source archive SHA-256, pinned by sherpa's build:
  `70cbf4050e7a014aae19140b05e57249da4720f56128459fbe3a93beaf971ae6`
- [Fork README and license declaration](https://github.com/csukuangfj/espeak-ng/blob/f6fed6c58b5e0998b8e68c6610125e2d07d595a7/README.md#license-information)
- `ESPEAK-NG-COPYING`: exact pinned GPLv3 text, SHA-256
  `8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903`
- The BSD2 appendix below: the fork's Windows getopt compatibility notice,
  included conservatively even when command-line compatibility code is unused.
- The Apache 2.0 text in `SHERPA-ONNX-LICENSE` and the Unicode appendix
  below retain the fork's additional Apache and Unicode data notices.

Sherpa's pinned `cmake/espeak-ng-for-piper.cmake` fetches the above source,
checks its hash, disables optional audio backends, and builds eSpeak NG as a
static library when building the shared Sherpa library. It passes that library
through Piper phonemization into the native Sherpa TTS code. Therefore a
separate eSpeak DLL need not appear in the installer for this component to be
present. The downloaded Windows shared archive contains eSpeak code and does
not contain a separate PortAudio DLL.

## Source and build access for these releases

The binary download and this notice are accompanied by the
[Comrade release page](https://github.com/karthik132007/Comrade/releases).
Select the same release tag recorded in `release-manifest.json` and download
GitHub's **Source code (zip)** or **Source code (tar.gz)** asset. That archive
contains Comrade's source, committed Cargo/npm lockfiles, native build and
packaging scripts, and GitHub Actions workflow for that tag. Direct source
archive URLs have this form, replacing `RELEASE_TAG` with that exact tag:

```text
https://github.com/karthik132007/Comrade/archive/refs/tags/RELEASE_TAG.zip
https://github.com/karthik132007/Comrade/archive/refs/tags/RELEASE_TAG.tar.gz
```

Native third-party sources and their integration/build instructions are
available without charge at these component locations:

1. [sherpa-onnx v1.12.9 source](https://github.com/k2-fsa/sherpa-onnx/archive/refs/tags/v1.12.9.tar.gz),
   including its CMake configuration, dependency pins and native build workflows.
2. [sherpa-rs-sys 0.6.8 source crate](https://static.crates.io/crates/sherpa-rs-sys/sherpa-rs-sys-0.6.8.crate),
   SHA-256 `591c9432b20f41d47f622a73c2888b188c321e313d820824df7d9fa51dbada43`.
   This is the exact source crate selected by Comrade's `Cargo.lock`. It includes
   the bundled Sherpa source/CMake integration, `build.rs` and `dist.json` used
   to select platform prebuilts. Use this crate's source when reproducing the
   Cargo integration, including its build adjustments; do not substitute an
   unpinned current branch.
3. The exact eSpeak NG source archive linked above, including its fork changes,
   per-file copyright/license notices and CMake build configuration.
4. Piper, sentencepiece and Kaldi source archives and licenses listed below;
   Sherpa's `cmake/` directory pins these and the other build dependencies.

On the native platform, install the toolchain and prerequisites described in
`docs/RELEASE.md`, then run `npm ci` and the matching command:

```sh
npm run release:linux
npm run release:desktop -- --target x86_64-pc-windows-msvc
npm run release:desktop -- --target aarch64-apple-darwin
npm run release:desktop -- --target x86_64-apple-darwin
```

Run only the command matching the host architecture/platform. Comrade's
release script builds the Rust application using its locked native dependency;
that dependency selects the pinned Sherpa prebuilt from `dist.json`. For a
native dependency source rebuild, consult the source crate's `build.rs` and
Sherpa's CMake/native workflows rather than treating a prebuilt download as a
source build. The sources above are the build inputs and instructions; the
release does not promise byte-identical compiler output across different hosts.

These directions identify the corresponding-source locations next to the
object-code downloads, including externally hosted component source. GPLv3
section 6(d), reproduced in `ESPEAK-NG-COPYING`, describes network distribution
and equivalent source access. The distributor remains responsible for keeping
that access available and for meeting the applicable license terms.

## Other native dependencies

- **Piper phonemize**, commit `78a788e0b719013401572d70fef372e77bff8e43`,
  Copyright (c) 2023 Michael Hansen, MIT: the Piper appendix below.
  [Pinned source](https://github.com/csukuangfj/piper-phonemize/archive/78a788e0b719013401572d70fef372e77bff8e43.zip),
  SHA-256 `89641a46489a4898754643ce57bda9c9b54b4ca46485fdc02bf0dc84b866645d`.
  Its bundled uni-algo notice appears in the uni-algo appendix below.
- **simple-sentencepiece v0.7**, Apache 2.0: the Apache 2.0 text in `SHERPA-ONNX-LICENSE`.
  [Pinned source](https://github.com/pkufool/simple-sentencepiece/archive/refs/tags/v0.7.tar.gz),
  SHA-256 `1748a822060a35baa9f6609f84efc8eb54dc0e74b9ece3d82367b7119fdc75af`.
- **kaldi-native-fbank v1.21.3**, Apache 2.0: the Apache 2.0 text in `SHERPA-ONNX-LICENSE`.
  [Pinned source](https://github.com/csukuangfj/kaldi-native-fbank/archive/refs/tags/v1.21.3.tar.gz),
  SHA-256 `d409eddae5a46dc796f0841880f489ff0728b96ae26218702cd438c28667c70e`.
- **kaldi-decoder v0.2.6**, Apache 2.0: the Apache 2.0 text in `SHERPA-ONNX-LICENSE`.
  [Pinned source](https://github.com/k2-fsa/kaldi-decoder/archive/refs/tags/v0.2.6.tar.gz),
  SHA-256 `b13c78b37495cafc6ef3f8a7b661b349c55a51abbd7f7f42f389408dcf86a463`.
- **sherpa-onnx v1.12.9**, Apache 2.0: `SHERPA-ONNX-LICENSE`.
- **ONNX Runtime 1.17.1**, MIT and third-party terms:
  `ONNXRUNTIME-LICENSE` and `ONNXRUNTIME-ThirdPartyNotices.txt`.
  [Pinned source](https://github.com/microsoft/onnxruntime/tree/v1.17.1).

The release packaging supplies the named Sherpa/ONNX Runtime license files
alongside this repository's notices. Optional models downloaded later can
carry their own licenses; consult the source/model download before use.

## eSpeak NG Windows getopt: COPYING.BSD2

```text
Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:
1. Redistributions of source code must retain the above copyright
  notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
  notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS
BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

## eSpeak NG Unicode data: COPYING.UCD

```text
Unicode Data Files include all data files under the directories
http://www.unicode.org/Public/, http://www.unicode.org/reports/,
http://www.unicode.org/cldr/data/, http://source.icu-project.org/repos/icu/, and
http://www.unicode.org/utility/trac/browser/.

Unicode Data Files do not include PDF online code charts under the
directory http://www.unicode.org/Public/.

Software includes any source code published in the Unicode Standard
or under the directories
http://www.unicode.org/Public/, http://www.unicode.org/reports/,
http://www.unicode.org/cldr/data/, http://source.icu-project.org/repos/icu/, and
http://www.unicode.org/utility/trac/browser/.

NOTICE TO USER: Carefully read the following legal agreement.
BY DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING UNICODE INC.'S
DATA FILES ("DATA FILES"), AND/OR SOFTWARE ("SOFTWARE"),
YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT.
IF YOU DO NOT AGREE, DO NOT DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE
THE DATA FILES OR SOFTWARE.

COPYRIGHT AND PERMISSION NOTICE

Copyright © 1991-2018 Unicode, Inc. All rights reserved.
Distributed under the Terms of Use in http://www.unicode.org/copyright.html.

Permission is hereby granted, free of charge, to any person obtaining
a copy of the Unicode data files and any associated documentation
(the "Data Files") or Unicode software and any associated documentation
(the "Software") to deal in the Data Files or Software
without restriction, including without limitation the rights to use,
copy, modify, merge, publish, distribute, and/or sell copies of
the Data Files or Software, and to permit persons to whom the Data Files
or Software are furnished to do so, provided that either
(a) this copyright and permission notice appear with all copies
of the Data Files or Software, or
(b) this copyright and permission notice appear in associated
Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT OF THIRD PARTY RIGHTS.
IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS
NOTICE BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL
DAMAGES, OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE,
DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
PERFORMANCE OF THE DATA FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder
shall not be used in advertising or otherwise to promote the sale,
use or other dealings in these Data Files or Software without prior
written authorization of the copyright holder.
```

## Piper phonemize: LICENSE.md

```text
MIT License

Copyright (c) 2023 Michael Hansen

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Piper bundled uni-algo: licenses/uni-algo/LICENSE.md

```text
Public Domain License

This is free and unencumbered software released into the public domain.

Anyone is free to copy, modify, publish, use, compile, sell, or distribute this
software, either in source code form or as a compiled binary, for any purpose,
commercial or non-commercial, and by any means.

In jurisdictions that recognize copyright laws, the author or authors of this
software dedicate any and all copyright interest in the software to the public
domain. We make this dedication for the benefit of the public at large and to
the detriment of our heirs and successors. We intend this dedication to be an
overt act of relinquishment in perpetuity of all present and future rights to
this software under copyright law.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN
ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
