export namespace Shapes {
  export interface Shape {
    area(): number;
  }

  export class Circle implements Shape {
    constructor(public radius: number) {}
    area(): number {
      return Math.PI * this.radius ** 2;
    }
  }

  const hidden = 1;
  export function unit(): Circle {
    return new Circle(hidden);
  }
}

export namespace Outer.Inner {
  export const depth = 2;
}

namespace Local {
  export const value = 3;
}

export const total = Shapes.unit().area() + Outer.Inner.depth + Local.value;
