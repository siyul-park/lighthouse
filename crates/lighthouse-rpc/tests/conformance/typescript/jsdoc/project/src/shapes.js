/**
 * @typedef {object} Point
 * @property {number} x
 * @property {number} y
 */

/**
 * Moves a point.
 * @param {Point} point the point
 * @param {number} dx
 * @returns {Point}
 */
export function move(point, dx) {
  return { x: point.x + dx, y: point.y };
}

export class Grid {
  /** @param {number} size */
  constructor(size) {
    /** @type {Point[]} */
    this.cells = [];
    this.size = size;
  }

  /** @returns {number} */
  count() {
    return this.cells.length;
  }
}

export const origin = move({ x: 0, y: 0 }, 1);
