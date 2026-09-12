"""Check every CUDA forwarding declaration against the pinned NVIDIA typedefs."""
import argparse
import hashlib
import json
from pathlib import Path
import re

WORKSPACE = Path(__file__).resolve().parents[2]
TYPES = {
    'void': 'c_void', 'char': 'c_char', 'unsigned char': 'u8', 'unsigned short': 'u16',
    'unsigned int': 'u32', 'int': 'i32', 'float': 'f32', 'size_t': 'usize', 'CUresult': 'CuResult',
    'CUdevice_v1': 'CuDevice', 'CUdevice': 'CuDevice', 'CUdeviceptr_v2': 'CuDevicePtr',
    'CUcontext': 'CuContext', 'CUstream': 'CuStream', 'CUevent': 'CuEvent', 'CUmodule': 'CuModule',
    'CUfunction': 'CuFunction', 'CUuuid': 'CuUuid', 'CUexecAffinityParam': 'CuExecAffinityParam',
    'CUdevice_attribute': 'i32', 'CUlimit': 'i32', 'CUjit_option': 'i32',
    'CUfunction_attribute': 'i32', 'CUfunc_cache': 'i32',
    'CUdriverProcAddressQueryResult': 'i32', 'cuuint64_t': 'u64',
}


def rust_type(parameter):
    match = re.fullmatch(r'(.*?)\b(\w+)', parameter.strip())
    if not match:
        raise ValueError(f'Unrecognized CUDA parameter {parameter}')
    c_type, _ = match.groups()
    constant = c_type.strip().startswith('const ')
    c_type = c_type.replace('const ', '').strip()
    result = TYPES[c_type.replace('*', '').strip()]
    for index in range(c_type.count('*')):
        result = ('*const ' if constant and index == 0 else '*mut ') + result
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--headers', type=Path, default=WORKSPACE / 'artifacts/cuda-sdk')
    args = parser.parse_args()
    lock = json.loads(Path(__file__).with_name('sdk-lock.json').read_text(encoding='utf-8'))
    for name, expected in lock['headers'].items():
        if hashlib.sha256((args.headers / name).read_bytes()).hexdigest() != expected:
            raise ValueError(f'CUDA header differs from the reviewed source: {name}')
    header = (args.headers / 'cudaTypedefs.h').read_text(encoding='utf-8')
    source = (WORKSPACE / 'libs/libcuda/src/api.rs').read_text(encoding='utf-8')
    declarations = re.findall(r'\[(\w+),\s*(\d+),\s*(\d+)\]\s*fn\s+(\w+)\((.*?)\);', source, re.S)
    if not declarations:
        raise ValueError('No forwarding declarations found')
    names = set()
    for base, version, flags, export, parameters in declarations:
        if export in names:
            raise ValueError(f'Duplicate CUDA entry {export}')
        names.add(export)
        suffix = '_ptsz' if export.endswith('_ptsz') else '_ptds' if export.endswith('_ptds') else ''
        undecorated = export[:-len(suffix)] if suffix else export
        if re.sub(r'_v\d+$', '', undecorated) != base:
            raise ValueError(f'CUDA query name differs from its exported function: {export}')
        if int(flags) != (2 if suffix else 0):
            raise ValueError(f'Wrong stream mode for {export}')
        signature = re.search(r'typedef CUresult \(CUDAAPI \*PFN_' + re.escape(base) + '_v' + version
                              + suffix + r'\)\(([^;]+)\);', header)
        if not signature:
            raise ValueError(f'No NVIDIA ABI anchor for {export} v{version}{suffix}')
        c_parameters = signature.group(1)
        expected = [] if c_parameters == 'void' else [rust_type(p) for p in c_parameters.split(',')]
        actual = [p.split(':', 1)[1].strip() for p in parameters.split(',') if p.strip()]
        if expected != actual:
            raise ValueError(f'CUDA ABI mismatch for {export}: expected {expected}, got {actual}')
    for filename, export, base, version in (
        ('dispatch.rs', 'cuGetProcAddress', 'cuGetProcAddress', 11030),
        ('dispatch.rs', 'cuGetProcAddress_v2', 'cuGetProcAddress', 12000),
        ('module.rs', 'cuModuleLoad', 'cuModuleLoad', 2000),
    ):
        body = (WORKSPACE / 'libs/libcuda/src' / filename).read_text(encoding='utf-8')
        parameters = re.search(r'pub unsafe extern "sysv64" fn ' + export
                               + r'\((.*?)\)\s*->\s*CuResult', body, re.S)
        signature = re.search(r'typedef CUresult \(CUDAAPI \*PFN_' + base + '_v' + str(version)
                              + r'\)\(([^;]+)\);', header)
        if not parameters or not signature:
            raise ValueError(f'Missing manual CUDA bridge signature: {export}')
        expected = [rust_type(p) for p in signature.group(1).split(',')]
        actual = [p.split(':', 1)[1].strip() for p in parameters.group(1).split(',') if p.strip()]
        if expected != actual:
            raise ValueError(f'CUDA ABI mismatch for {export}: expected {expected}, got {actual}')
        names.add(export)
    print(f'Verified {len(names)} CUDA signatures, including query and VFS module bridges')


if __name__ == '__main__':
    main()
