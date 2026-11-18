type Field = { name: string; pattern: string; value: string };

self.onmessage = ({ data }: MessageEvent<Field[]>) => {
  const mismatches = data.flatMap(({ name, pattern, value }) => {
    try {
      return new RegExp(pattern).test(value) ? [] : [name];
    } catch {
      return [];
    }
  });
  self.postMessage(mismatches);
};
