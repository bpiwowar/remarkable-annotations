# remarkable-annotations (npm)

Read and write the annotations of reMarkable documents (`.rm` v6 scene
files), compiled to WebAssembly from the
[remarkable-annotations](https://github.com/bpiwowar/remarkable-annotations)
Rust library.

```js
import init, { Scene } from "remarkable-annotations";

await init(); // or initSync({ module: bytesOfTheWasmFile })
const scene = new Scene(rmBytes);
const blocks = JSON.parse(scene.blocksJson());
const same = scene.serialize(); // byte for byte
```

The package is an ES module built with `wasm-pack --target web`. In
environments without a URL for the `.wasm` file (privileged Firefox code such
as a Zotero plugin), fetch its bytes and call `initSync({ module: bytes })`.
