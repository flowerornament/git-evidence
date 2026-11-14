{ lib, stdenv, rustPlatform, git, jujutsu, makeWrapper, pkg-config, sqlite }:
let
  version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
in rustPlatform.buildRustPackage {
  pname = "git-evidence";
  inherit version;
  src = lib.fileset.toSource {
    root = ./.;
    # The root README is a test input: the crate README must open as it does.
    fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./rust-toolchain.toml ./README.md ./crates ];
  };
  cargoLock.lockFile = ./Cargo.lock;
  cargoBuildFlags = [ "--package" "git-evidence" ];
  # Both crates' tests, with the test-only features, as `just check` runs them.
  cargoTestFlags = [ "--workspace" "--all-features" ];
  # That test runs the audit under launchd's PATH (/usr/bin:/bin) and needs
  # uname there; the Linux build sandbox has no /usr/bin or /bin. `just check`
  # runs it on Linux and macOS.
  checkFlags = lib.optionals stdenv.hostPlatform.isLinux [
    "--skip=the_audit_reads_this_machines_names_under_a_launchd_path"
  ];
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
