#!/usr/bin/env python3
"""Compile current memefs methods with portable OS stubs; no Windows SDK required."""
import os
from pathlib import Path
import re
import shlex
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
VENDOR = ROOT / 'vendor/memefs'
HERE = Path(__file__).resolve().parent


def source(name):
    text = (VENDOR / name).read_text(encoding='utf-8-sig')
    if name == 'exceptions.h':
        # MSVC accepts these legacy overrides; libstdc++ requires noexcept.
        text = text.replace('what() const override', 'what() const noexcept override')
    return re.sub(r'^\s*#(?:include|pragma once)[^\n]*\n', '', text, flags=re.M)


def method(name, signature):
    """Extract an entire definition, retaining production code instead of a test copy."""
    text = source(name)
    start = text.index(signature)
    opening = text.index('{', start)
    # Ignore literals and comments without changing offsets into the source.
    masked = re.sub(r'//[^\n]*|/\*.*?\*/|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'',
                    lambda match: ' ' * len(match.group()), text, flags=re.S)
    depth = 1
    end = opening + 1
    while depth:
        depth += (masked[end] == '{') - (masked[end] == '}')
        end += 1
    return text[start:end] + '\n'


parts = [(HERE / 'platform.h').read_text()]
parts += [source(name) for name in ['globalincludes.h', 'comparisons.h', 'utils.h',
                                  'sectors.h', 'dynamicstruct.h', 'exceptions.h',
                                  'nodes.h', 'memfs.h']]
parts += [(HERE / 'support.h').read_text()]
parts += [source(name) for name in ['comparisons.cpp', 'nodes.cpp', 'sectors.cpp', 'filemap.cpp']]
parts += ['namespace Memfs::Utils {', method('utils.cpp', 'SuffixView PathSuffix('), '}']
parts += ['namespace Memfs {', method('nodes-compat.cpp', 'NTSTATUS CompatSetFileSizeInternal('),
          method('nodes-compat.cpp', 'NTSTATUS CompatFspFileNodeSetEa('), '}']
parts += ['namespace Memfs::Interface {', method('filecreate.cpp', 'NTSTATUS Create('),
          method('fileinfo.cpp', 'NTSTATUS Rename('),
          method('fileinfo.cpp', 'NTSTATUS SetFileSize('),
          method('io.cpp', 'NTSTATUS Write('), method('io.cpp', 'NTSTATUS Read('), '}']
parts += [(HERE / 'regressions.cpp').read_text()]

with tempfile.TemporaryDirectory(prefix='memefs-native-') as tmp:
    cpp = Path(tmp) / 'regressions.cpp'
    exe = Path(tmp) / 'regressions'
    cpp.write_text('\n'.join(parts))
    compiler = shlex.split(os.environ.get('CXX', 'clang++'))
    flags = shlex.split(os.environ.get('NATIVE_CXXFLAGS', '-fsanitize=address,undefined -fno-omit-frame-pointer'))
    subprocess.run(compiler + ['-std=c++20', '-g', '-O1', '-pthread', '-DMEMFS_DIAGNOSTICS=1'] +
                   flags + [str(cpp), '-o', str(exe)], check=True)
    subprocess.run([str(exe)], check=True)
