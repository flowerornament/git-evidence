{
  description = "git-evidence: numbers recorded per Git commit, kept in Git notes";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.rust-overlay.url = "github:oxalica/rust-overlay";
  inputs.rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      eachSystem = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; overlays = [ rust-overlay.overlays.default ]; };
      # The toolchain rust-toolchain.toml pins, for the shell and the package alike.
      rustFor = pkgs: pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
    in {
      # The `git-evidence` binary, wrapped to find Git, with its manual pages.
      packages = eachSystem (system:
        let
          pkgs = pkgsFor system;
          rust = rustFor pkgs;
        in {
          default = pkgs.callPackage ./package.nix {
            rustPlatform = pkgs.makeRustPlatform { cargo = rust; rustc = rust; };
          };
        });
      apps = eachSystem (system: {
        default = { type = "app"; program = "${self.packages.${system}.default}/bin/git-evidence"; };
      });
      checks = eachSystem (system: { package = self.packages.${system}.default; });
      # Everything `just check` needs: the pinned toolchain, Git (2.42 or later),
      # jj (the revision tests drive jj workspaces) and just.
      devShells = eachSystem (system:
        let pkgs = pkgsFor system; in {
          default = pkgs.mkShell {
            packages = [ (rustFor pkgs) pkgs.git pkgs.jujutsu pkgs.just ];
            # Git never discovers a repository above the checkout (a stray ~/.git).
            shellHook = ''export GIT_CEILING_DIRECTORIES="$(dirname "$PWD")"'';
          };
        });
    };
}
