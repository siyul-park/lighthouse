export class Owner {
  public open = 1;
  private secret = 2;
  #hidden = 3;
  protected shared = 4;
  readonly fixed = 5;
  static count = 0;

  constructor(private readonly dep: string, public name: string, plain: number) {
    void plain;
  }

  private help(): number {
    return this.secret + this.#hidden;
  }

  protected inherit(): number {
    return this.help() + this.shared;
  }

  get total(): number {
    return this.inherit();
  }

  set total(value: number) {
    this.secret = value;
  }
}

class Internal {
  member = 1;
}

export const reach = new Internal().member;
export { Internal as Renamed };

const local = 1;
export default local;
