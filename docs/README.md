# XUVA documentation

This index keeps the public README focused on product behavior while preserving
the evidence, contracts, and milestone records behind it.

## Start here

| Need | Document |
| --- | --- |
| Understand adaptive route selection and configuration | [XUVA contract](XUVA.md) |
| Compare raw Windows, stock RTK, and XUVA honestly | [P20 benchmark comparison](BENCHMARK_COMPARISON_P20.md) |
| Inspect exact native Windows, WSL1, and WSL2 figures | [P18 core matrix](BENCHMARK_CORE_MATRIX_P18_2026-07-25.md) |
| Verify a source checkout before release | [P20 release gate](RELEASE_GATE_P20.md) |
| Inspect the v0.3.0 baseline and P7 dispatcher boundary | [P7 dispatcher foundation](P7_DISPATCHER_FOUNDATION.md) |
| Install the private tokenizer dependency | [Dependencies](DEPENDENCIES.md) |

## Cross-host providers and setup

The provider sequence is intentionally progressive: inspect an existing
provider, prove mapping, execute explicitly, then consider separately confirmed
setup.

1. [PD1 discovery](PROVIDER_DISCOVERY_PD1.md)
2. [PD2 provider resolution](PROVIDER_RESOLUTION_PD2.md)
3. [PD3 provider-aware execution](PROVIDER_EXECUTION_PD3.md)
4. [PD4 assisted setup planning](ASSISTED_SETUP_PD4.md)
5. [PD5 opt-in setup](OPT_IN_SETUP_PD5.md)
6. [PD6 operational freeze](SETUP_OPERATIONAL_FREEZE_PD6.md)
7. [P11 provider baseline](PROVIDER_BASELINE_P11.md)
8. [P12 generic registry](GENERIC_PROVIDER_REGISTRY_P12.md)
9. [P13 bidirectional mapping](BIDIRECTIONAL_PROVIDER_MAPPING_P13.md)
10. [P14 generic execution](PROVIDER_EXECUTION_ENGINE_P14.md)
11. [P17 generic setup diagnosis](GENERIC_SETUP_DIAGNOSIS_P17.md)
12. [P19 on-demand cross-host providers](CROSS_HOST_ON_DEMAND_P19.md)

## Benchmarks and dogfooding

- [Core Windows/WSL benchmark matrix](BENCHMARK_CORE_MATRIX_P18_2026-07-25.md)
- [Public comparison with explicit token savings](BENCHMARK_COMPARISON_P20.md)
- [Flowpeek three-way benchmark](BENCHMARK_FLOWPEEK_2026-07-24.md)
- [Cargo check benchmark](BENCHMARK_CARGO_CHECK_2026-07-24.md)
- [NPM run-list benchmark](BENCHMARK_NPM_RUN_LIST_2026-07-24.md)
- [Go, Dart, and Flutter benchmark](BENCHMARK_GO_DART_FLUTTER_2026-07-24.md)
- [External fixture validation](BENCHMARK_EXTERNAL_FIXTURES_2026-07-24.md)
- [Filesystem matrix](FILESYSTEM_MATRIX_2026-07-24.md)
- [Dogfood cycles](DOGFOOD_CYCLES_2026-07-24.md)

## Release and project history

- [Alpha stabilization](ALPHA_STABILIZATION.md)
- [Alpha release checklist](ALPHA_RELEASE_CHECKLIST.md)
- [P20 freeze](P20_FREEZE.md)
- [Upstream proposal](UPSTREAM_PROPOSAL.md)
- [WSL1 bridge](WSL1_BRIDGE.md) and [validation](WSL1_VALIDATION.md)
- [Command-surface parity](COMMAND_SURFACE_PARITY_P15.md)
- [Adaptive decision hardening](ADAPTIVE_DECISION_HARDENING_P16.md)
- [Tokenizer bootstrap](TOKENIZER_BOOTSTRAP_P19.md)
