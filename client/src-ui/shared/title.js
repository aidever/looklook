// 标签页标题前的标记：对话页在后台有事时加的 `● `，等你处理的个数 `(2) `（App.jsx useTitleCount）。
// 两处各改各的，都要先认出全部前缀再拼，不然会一层层叠上去（`● (1) ● (1) 看看`）。

/** `{ dot, base }`：有没有 `● `、去掉所有前缀后的标题。 */
export function splitTitle(title) {
  const m = /^((?:● |\(\d+\) )*)/.exec(title);
  return { dot: m[1].includes('●'), base: title.slice(m[1].length) };
}
