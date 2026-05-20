{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  buildInputs = [
    pkgs.rustup
    pkgs.typescript
    pkgs.typescript-language-server
  ];
}
