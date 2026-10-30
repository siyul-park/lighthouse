export function shapes(n: number, items: number[], name?: string): number {
  let total = 0;
  if (n > 0) {
    total += 1;
  } else if (n < 0) {
    total -= 1;
  } else {
    total = 0;
  }
  for (const item of items) {
    if (item > 1 && item < 10 || item === 0) {
      continue;
    }
    total += item;
  }
  outer: for (let i = 0; i < 3; i++) {
    for (let j = 0; j < 3; j++) {
      if (i === j) break outer;
    }
  }
  while (total > 100) total /= 2;
  do {
    total++;
  } while (total < 3);
  switch (n) {
    case 1:
      total += 1;
      break;
    case 2:
    case 3:
      total += 2;
      break;
    default:
      total = 0;
  }
  try {
    total += JSON.parse("1");
  } catch (error) {
    total = -1;
  } finally {
    total += 1;
  }
  const label = name ?? (total > 3 ? "big" : "small");
  items.forEach((item) => {
    if (item) total += item;
  });
  return label.length + total;
}

export function pick(kind: string): number {
  switch (kind) {
    case "a":
      return 1;
    case "b":
      return 2;
    default:
      return 0;
  }
}

export function fact(n: number): number {
  return n <= 1 ? 1 : n * fact(n - 1);
}

export const arrow = (a: number) => a + 1;
