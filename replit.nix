{ pkgs }: {
	deps = [
		pkgs.unixtools.xxd
  pkgs.rustc
		pkgs.rustfmt
		pkgs.cargo
		pkgs.cargo-edit
        pkgs.rust-analyzer
        pkgs.flatbuffers
	];
}