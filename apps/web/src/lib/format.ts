export function formatDate(value?: string | null): string {
  if (!value) return "Not available";
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) return value;
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

export function formatRelative(value?: string | null): string {
  if (!value) return "Unknown";
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) return value;
  const delta = date.valueOf() - Date.now();
  const minutes = Math.round(delta / 60_000);
  if (Math.abs(minutes) < 1) return "Now";
  if (Math.abs(minutes) < 60) return `${Math.abs(minutes)}m ${minutes < 0 ? "ago" : "ahead"}`;
  const hours = Math.round(minutes / 60);
  if (Math.abs(hours) < 48) return `${Math.abs(hours)}h ${hours < 0 ? "ago" : "ahead"}`;
  const days = Math.round(hours / 24);
  return `${Math.abs(days)}d ${days < 0 ? "ago" : "ahead"}`;
}

export function formatBytes(value?: number): string {
  if (value === undefined) return "Unknown";
  if (value < 1024) return `${value} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let amount = value / 1024;
  let unit = units[0];
  for (let index = 1; amount >= 1024 && index < units.length; index += 1) {
    amount /= 1024;
    unit = units[index];
  }
  return `${amount.toFixed(amount >= 10 ? 0 : 1)} ${unit}`;
}

export function shortId(value?: string | null, size = 10): string {
  if (!value) return "None";
  if (value.length <= size + 3) return value;
  return `${value.slice(0, size)}...`;
}

export function humanize(value?: string): string {
  if (!value) return "Unknown";
  return value
    .replace(/[._-]+/g, " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}
