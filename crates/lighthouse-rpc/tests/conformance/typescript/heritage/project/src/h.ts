export interface Animal {
  speak(): string;
}

export abstract class Base implements Animal {
  abstract speak(): string;

  describe(): string {
    return this.speak();
  }
}

export class Dog extends Base {
  speak(): string {
    return "woof";
  }

  override describe(): string {
    return super.describe() + "!";
  }

  forward(text: string): string {
    return this.wrap(text);
  }

  private wrap(text: string): string {
    return text;
  }

  static make(): Dog {
    return new Dog();
  }
}

export function build(): Dog {
  return Dog.make();
}

export function forwarded(a: number, b: number): number {
  return add(a, b);
}

function add(a: number, b: number): number {
  return a + b;
}
