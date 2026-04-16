{
  description = "Embers development shell with pinned flatc";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { nixpkgs, ... }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];

      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system:
          f (import nixpkgs { inherit system; }));
    in
    {
      packages = forAllSystems (pkgs: {
        flatc = pkgs.flatbuffers;
        default = pkgs.flatbuffers;
      });

      apps = forAllSystems (pkgs: {
        flatc = {
          type = "app";
          program = "${pkgs.flatbuffers}/bin/flatc";
        };
        default = {
          type = "app";
          program = "${pkgs.flatbuffers}/bin/flatc";
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            pkgs.flatbuffers
            pkgs.mdbook
          ];
        };
      });
    };
}
