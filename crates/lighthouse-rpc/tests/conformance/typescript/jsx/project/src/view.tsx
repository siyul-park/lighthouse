export function Greeting(props: { name: string }) {
  return <p>Hello {props.name}</p>;
}

export const Layout = {
  Header: (props: { title: string }) => <h1>{props.title}</h1>,
};

export class Page {
  render() {
    return (
      <>
        <Layout.Header title="x" />
        <Greeting name="y" />
      </>
    );
  }
}
