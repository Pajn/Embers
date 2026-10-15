{
  description = "Embers development shell with pinned flatc, Zig and bindgen";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  # Zig and bindgen for embers-ghostty-sys, pinned apart from `nixpkgs` so
  # updating them leaves flatc and mdbook, whose output is checked in, as
  # they are.
  inputs.nixpkgs-ghostty.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { nixpkgs, nixpkgs-ghostty, ... }:
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

      devShells = forAllSystems (pkgs:
        let
          ghosttyPkgs = import nixpkgs-ghostty { inherit (pkgs.stdenv.hostPlatform) system; };
        in
        {
          default = pkgs.mkShell {
            packages = [
              pkgs.flatbuffers
              pkgs.mdbook
              # embers-ghostty-sys: Zig builds libghostty-vt, bindgen
              # regenerates its bindings.
              ghosttyPkgs.zig_0_16
              ghosttyPkgs.rust-bindgen
            ]
            # libghostty-vt's macOS build edits its archive with nmedit.
            ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
              ghosttyPkgs.darwin.cctools
            ];
          };
        });
    };
}
