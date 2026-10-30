export interface Config {
  name: string;
  retries?: number;
  parent: Config | null;
  items: Config[];
}

type Local = { x: number };

export class Service {
  constructor(private config: Config) {}

  async load(id: string): Promise<Config> {
    return { ...this.config, name: id };
  }

  async touch(): Promise<void> {}

  find(local: Local, ...rest: string[]): Config | undefined {
    void local;
    void rest;
    return undefined;
  }

  static create(config: Config): Service {
    return new Service(config);
  }
}

export function build(config: Config): Service {
  return new Service(config);
}

export function fail(): never {
  throw new Error("x");
}
