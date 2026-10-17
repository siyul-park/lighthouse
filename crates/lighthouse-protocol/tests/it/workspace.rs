use std::path::Path;

use lighthouse_protocol::{Implementation, InitializeParams, VERSION, WorkspaceFolder};

fn params(folders: Vec<WorkspaceFolder>) -> InitializeParams {
    InitializeParams {
        protocol_version: VERSION.to_owned(),
        client_info: Implementation {
            name: "t".to_owned(),
            version: "0".to_owned(),
        },
        workspace_folders: folders,
    }
}

#[test]
fn a_folder_is_a_percent_encoded_file_uri_that_reads_back_as_the_path() {
    let path = Path::new("/home/ünï/a b#c%d");

    let folder = WorkspaceFolder::at(path).unwrap();

    assert_eq!(folder.uri, "file:///home/%C3%BCn%C3%AF/a%20b%23c%25d/");
    assert_eq!(folder.name, "a b#c%d");
    assert_eq!(folder.path().unwrap(), path);
}

#[test]
fn a_relative_path_is_no_folder() {
    assert!(WorkspaceFolder::at(Path::new("rel")).is_err());
}

#[test]
fn the_root_is_the_one_folder_and_nothing_else_is() {
    let folder = WorkspaceFolder::at(Path::new("/p")).unwrap();
    assert_eq!(
        params(vec![folder.clone()]).root().unwrap(),
        Path::new("/p")
    );

    for folders in [
        vec![],
        vec![folder.clone(), folder],
        vec![WorkspaceFolder {
            uri: "https://example.com/p".to_owned(),
            name: "p".to_owned(),
        }],
    ] {
        assert_eq!(params(folders).root().unwrap_err().code, -32602);
    }
}
