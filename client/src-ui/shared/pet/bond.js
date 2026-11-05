// 好感度等级：经验（xp）越多等级越高，越往后升得越慢。

/** 每级要的累计经验：Lv1 0、Lv2 20、Lv3 60、Lv4 150、Lv5 300、Lv6 600、Lv7 1000，之后每级 +600 */
const STEPS = [0, 20, 60, 150, 300, 600, 1000];

export function levelOf(xp = 0) {
  let level = 1;
  const need = (l) => (l <= STEPS.length ? STEPS[l - 1] : STEPS[STEPS.length - 1] + (l - STEPS.length) * 600);
  while (xp >= need(level + 1)) level += 1;
  const from = need(level);
  const to = need(level + 1);
  return { level, from, to, progress: (xp - from) / (to - from) };
}
