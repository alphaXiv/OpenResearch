export function downloadFile(fileName: string, file: Blob) {
  const url = URL.createObjectURL(file);
  const link = document.createElement("a");
  link.href = url;
  link.download = fileName;
  // Safari ignores a detached anchor and cancels a download whose blob is revoked too early.
  document.body.append(link);
  link.click();
  link.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}
