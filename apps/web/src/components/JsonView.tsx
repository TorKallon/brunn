import type { JsonValue } from "../lib/types";

export function JsonView({ value, label = "Structured data" }: { value: JsonValue; label?: string }) {
  return (
    <pre className="json-view" aria-label={label}>
      {JSON.stringify(value, null, 2)}
    </pre>
  );
}
