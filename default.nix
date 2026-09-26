{
  lib,
  rustPlatform,
  callPackage,
  runCommand,
  installShellFiles,
  git,
  gitRev ? null,
  grammarOverlays ? [ ],
  includeGrammarIf ? _: true,
}:
let
  fs = lib.fileset;

  src = fs.difference (fs.gitTracked ./.) (
    fs.unions [
      ./.envrc
      ./rustfmt.toml
      ./screenshot.png
      ./book
      ./docs
      ./runtime
      ./nix
      ./flake.lock
      (fs.fileFilter (file: lib.strings.hasInfix ".git" file.name) ./.)
      (fs.fileFilter (file: file.hasExt "svg") ./.)
      (fs.fileFilter (file: file.hasExt "md") ./.)
      (fs.fileFilter (file: file.hasExt "nix") ./.)
    ]
  );

  # Next we actually need to build the grammars and the runtime directory
  # that they reside in. It is built by calling the derivation in the
  # grammars.nix file, then taking the runtime directory in the git repo
  # and hooking symlinks up to it.
  grammars = callPackage ./nix/grammars.nix { inherit grammarOverlays includeGrammarIf; };
  runtimeDir = runCommand "mitos-runtime" { } ''
    mkdir -p $out
    ln -s ${./runtime}/* $out
    rm -r $out/grammars
    ln -s ${grammars} $out/grammars
  '';
in
rustPlatform.buildRustPackage (self: {
  postPatch = ''
    substituteInPlace crates/view/src/theme.rs \
      --replace-fail '../../../runtime/themes/base16_terminal.toml' '${./runtime/themes/base16_terminal.toml}'    
  '';
  
  cargoLock = {
    lockFile = ./Cargo.lock;
    # This is not allowed in nixpkgs but is very convenient here: it allows us to
    # avoid specifying `outputHashes` here for any git dependencies we might take
    # on temporarily.
    allowBuiltinFetchGit = true;
  };

  propagatedBuildInputs = [ runtimeDir ];

  nativeBuildInputs = [
    installShellFiles
    git
  ];

  buildType = "release";

  name = with builtins; (fromTOML (readFile ./crates/term/Cargo.toml)).package.name;
  src = fs.toSource {
    root = ./.;
    fileset = src;
  };

  # Mitos attempts to reach out to the network and get the grammars. Nix doesn't allow this.
  MITOS_DISABLE_AUTO_GRAMMAR_BUILD = "1";

  # So Mitos knows what rev it is.
  MITOS_NIX_BUILD_REV = gitRev;

  doCheck = false;
  strictDeps = true;

  # Sets the Mitos runtime dir to the grammars
  env.MITOS_DEFAULT_RUNTIME = "${runtimeDir}";

  # Get all the application stuff in the output directory.
  postInstall = ''
    mkdir -p $out/lib
    installShellCompletion ${./contrib/completion}/ms.{bash,fish,zsh}
    mkdir -p $out/share/applications
    cp ${./contrib/Mitos.desktop} $out/share/applications/Mitos.desktop
  '';

  meta.mainProgram = "ms";
})
