cd ../..
cargo build --release -p breakwater-parser-c-bindings
cd -
gcc test.c -o test -l breakwater_parser_c_bindings -L../../target/release

LD_LIBRARY_PATH=../../target/release ./test
