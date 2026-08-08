export type StreamingToolInput = Record<string, string>;

/**
 * Reads complete and in-progress top-level JSON string fields without requiring
 * the enclosing object or the current string value to be complete.
 */
export function parseStreamingToolInput(value: string): StreamingToolInput {
  const fields: StreamingToolInput = {};
  let index = value.indexOf("{") + 1;
  if (index === 0) return fields;

  while (index < value.length) {
    index = skipWhitespace(value, index);
    if (value[index] === "}") break;
    if (value[index] !== '"') break;

    const key = readJsonString(value, index);
    if (!key.complete) break;
    index = skipWhitespace(value, key.end);
    if (value[index] !== ":") break;
    index = skipWhitespace(value, index + 1);
    if (value[index] !== '"') break;

    const fieldValue = readJsonString(value, index);
    fields[key.value] = fieldValue.value;
    if (!fieldValue.complete) break;

    index = skipWhitespace(value, fieldValue.end);
    if (value[index] !== ",") break;
    index += 1;
  }

  return fields;
}

function skipWhitespace(value: string, index: number): number {
  while (index < value.length && /\s/.test(value[index])) index += 1;
  return index;
}

function readJsonString(
  value: string,
  start: number,
): { value: string; end: number; complete: boolean } {
  let result = "";
  let index = start + 1;
  while (index < value.length) {
    const character = value[index];
    if (character === '"') {
      return { value: result, end: index + 1, complete: true };
    }
    if (character !== "\\") {
      result += character;
      index += 1;
      continue;
    }

    const escaped = value[index + 1];
    if (escaped === undefined) break;
    const replacements: Record<string, string> = {
      '"': '"',
      "\\": "\\",
      "/": "/",
      b: "\b",
      f: "\f",
      n: "\n",
      r: "\r",
      t: "\t",
    };
    if (escaped === "u") {
      const hex = value.slice(index + 2, index + 6);
      if (!/^[0-9a-fA-F]{4}$/.test(hex)) break;
      result += String.fromCharCode(Number.parseInt(hex, 16));
      index += 6;
    } else {
      result += replacements[escaped] ?? escaped;
      index += 2;
    }
  }
  return { value: result, end: value.length, complete: false };
}
