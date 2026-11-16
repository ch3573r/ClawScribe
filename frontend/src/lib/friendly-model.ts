/** Short display names for model pills; retain the full ID in tooltips. */
export function friendlyModel(model?: string): string {
  const id = model?.trim();
  if (!id) return "Choose a model";
  const name = id.split("/").filter(Boolean).pop() ?? id;
  const claude = /^(?:claude[-:]?)?(sonnet|opus|haiku)(?:$|[-:])/i.exec(name);
  if (claude) return `Claude ${claude[1][0].toUpperCase()}${claude[1].slice(1).toLowerCase()}`;
  const gpt = /gpt[- ]?(\d+(?:\.\d+)?)/i.exec(name);
  if (gpt) return `GPT-${gpt[1]}`;
  const named = /^(qwen|llama|gemma|claude|gemini|deepseek)[-:]?([\d.]*)/i.exec(name);
  if (named) return `${named[1][0].toUpperCase()}${named[1].slice(1)}${named[2] ? ` ${named[2]}` : ""}`;
  // Repository-style ids stay in the tooltip, out of the normal view.
  return "Selected model";
}
