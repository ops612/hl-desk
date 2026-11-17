// PNG bytes from a Rust command (tauri::ipc::Response) as an object URL for <img>.
// The fast IPC path hands over an ArrayBuffer; when Tauri falls back to its postMessage path
// the same bytes arrive as a plain number array. A Blob made from that array would hold the
// text "137,80,78,..." instead of the image, so both shapes are turned into real bytes here.
export function pngUrl(data: unknown): string {
  let bytes: Uint8Array<ArrayBuffer>;
  if (data instanceof ArrayBuffer) bytes = new Uint8Array(data);
  else if (ArrayBuffer.isView(data))
    bytes = new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength) as ArrayBuffer);
  else if (Array.isArray(data)) bytes = Uint8Array.from(data as number[]);
  else throw new Error(`unexpected image data (${Object.prototype.toString.call(data)})`);
  return URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
}
