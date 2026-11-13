{ lib, rustPlatform, git, jujutsu, makeWrapper, pkg-config, sqlite }:
let
  version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
in rustPlatform.buildRustPackage {
  pname = "git-evidence";
  inherit version;
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./rust-toolchain.toml ./crates ];
  };
  cargoLock.lockFile = ./Cargo.lock;
  cargoBuildFlags = [ "--package" "git-evidence" ];
  # Both crates' tests, with the test-only features, as `just check` runs them.
  cargoTestFlags = [ "--workspace" "--all-features" ];
  # nixpkgs' cargo hook links libsqlite3-sys against the system SQLite
  # (LIBSQLITE3_SYS_USE_PKG_CONFIG=1) in place of rusqlite's bundled copy.
  nativeBuildInputs = [ makeWrapper pkg-config ];
  buildInputs = [ sqlite ];
  # The revision tests drive real jj workspaces.
  nativeCheckInputs = [ git jujutsu ];
  doCheck = true;
  postInstall = ''
    mkdir -p $out/share/man/man1
    $out/bin/git-evidence manual $out/share/man/man1
    wrapProgram $out/bin/git-evidence --prefix PATH : ${lib.makeBinPath [ git ]}
  '';
  doInstallCheck = true;
  # The manual pages are gzipped by the fixup phase.
  installCheckPhase = ''
    runHook preInstallCheck
    test "$($out/bin/git-evidence --version)" = "git-evidence ${version}"
    $out/bin/git-evidence --help >/dev/null
    test -s $out/share/man/man1/git-evidence.1.gz
    test -s $out/share/man/man1/git-evidence-record.1.gz
    runHook postInstallCheck
  '';
  meta = {
    description = "Numbers recorded per Git commit, kept in Git notes";
    homepage = "https://github.com/flowerornament/git-evidence";
    license = with lib.licenses; [ mit asl20 ];
    mainProgram = "git-evidence";
    platforms = lib.platforms.unix;
  };
}
