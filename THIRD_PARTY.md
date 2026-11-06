# Third-party notices

deem embeds or redistributes the following, each under the MIT license.

## jaq

deem's jq engine is [jaq](https://github.com/01mf02/jaq) (`jaq-core`,
`jaq-std`, `jaq-json`), by Michael Färber, used as a library at pinned
versions. jaq is under the MIT license below; its license file has no
copyright line.

## jq

`tests/golden/jq.test` is jq 1.7.1's test suite, from
<https://github.com/jqlang/jq/blob/jq-1.7.1/tests/jq.test>, used as a corpus
of programs. `tests/golden/everyday.out` records jq 1.7.1's output on
deem's own corpus; `tests/golden/regen.sh` downloads the jq 1.7.1 binary
into `target/` to produce it, and it is not distributed. jq is by Stephen
Dolan and the jq contributors, under the MIT license below, with this
notice (<https://github.com/jqlang/jq/blob/jq-1.7.1/COPYING>):

    jq is copyright (C) 2012 Stephen Dolan

`src/engine/compat.jq` restates definitions from jq 1.7.1's `builtin.jq` for
builtins jaq lacks.

## MIT license

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
