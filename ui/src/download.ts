export function fileNameOf(path: string) {
  return path.split(/[\\/]/).pop()?.split(/[?#]/)[0] ?? "";
}

export function downloadUrl(fileName: string, url: string) {
  const link = document.createElement("a");
  link.href = url;
  link.download = fileName;
  // Safari ignores a detached anchor.
  document.body.append(link);
  link.click();
  link.remove();
}

export function downloadFile(fileName: string, file: Blob) {
  const url = URL.createObjectURL(file);
  downloadUrl(fileName, url);
  // Safari cancels a download whose blob is revoked too early.
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}
