#!/usr/bin/env python3
"""Linux sample qualification. Never use original data as a destructive test root."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import sys
import time

REPO = Path(__file__).resolve().parent.parent
MIB = 1024**2
MARKER = ".keeper-qualification.json"


def save(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=True) + "\n")


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(MIB):
            h.update(chunk)
    return h.hexdigest()


def snapshot(root):
    """Hash authoritative metadata with bounded buffering, not read_bytes()."""
    if not root.exists():
        return None
    result = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"metadata snapshot refuses symlink: {path}")
        if path.is_file():
            result[str(path.relative_to(root))] = digest(path)
    return result


def fingerprint(meta):
    return [meta.st_dev, meta.st_ino, meta.st_size, meta.st_mtime_ns, meta.st_ctime_ns]


def source_metadata(source):
    ignore = source / ".keeperignore"
    return {"catalog": snapshot(source / ".keeper/catalog"),
            "ignore": digest(ignore) if ignore.exists() else None}


def new_path(value):
    path = Path(value).absolute()
    if path.exists() or path.is_symlink():
        raise ValueError(f"destination must not exist: {path}")
    parent = path.parent.resolve(strict=True)
    return parent / path.name


def select(candidates, budget, max_files):
    items = sorted(candidates.items(), key=lambda x: hashlib.sha256(os.fsencode(x[0])).digest())
    chosen = []
    used = 0
    for lo, hi, count, cap in [(0, 32*1024, 1000, budget),
                               (32*1024+1, 8*MIB, 150, min(budget, 256*MIB)),
                               (8*MIB+1, budget, max_files, budget)]:
        eligible = [x for x in items if lo <= x[1] <= hi]
        if lo > 8*MIB:
            eligible.sort(key=lambda x: -x[1])
        added = 0
        for item in eligible:
            if len(chosen) >= max_files or added >= count:
                break
            if used + item[1] <= cap:
                chosen.append(item)
                used += item[1]
                added += 1
    return sorted(chosen)


def prepare(args):
    source = Path(args.source).resolve(strict=True)
    workspace, report = new_path(args.workspace), new_path(args.report)
    if not source.is_dir():
        raise ValueError("source must be a directory")
    if source.stat().st_dev != workspace.parent.stat().st_dev:
        raise ValueError("workspace must be on the source filesystem")
    if (report.is_relative_to(source) or source.is_relative_to(report)
            or report.is_relative_to(workspace) or workspace.is_relative_to(report)
            or workspace.is_relative_to(source / '.keeper')):
        raise ValueError("report must be outside source/workspace; workspace must not be inside .keeper")
    if shutil.disk_usage(workspace.parent).free < 2*args.budget_mib*MIB + 4*1024*MIB:
        raise ValueError("insufficient free space for both samples and interruption tests")
    workspace.mkdir(mode=0o700)
    report.mkdir(mode=0o700)
    for name in ["sample-a", "sample-b", "test-tmp"]:
        (workspace / name).mkdir()
    manifest = {"version": 1, "workspace": str(workspace), "report": str(report),
                "source": str(source), "workspace_binding": fingerprint(workspace.stat())[:2],
                "source_binding": fingerprint(source.stat())[:2],
                "source_metadata": source_metadata(source), "state": "preparing"}
    save(workspace / MARKER, manifest)
    candidates, scanned = {}, {}
    for subtree in args.subtree or ["."]:
        start = source / subtree
        if (not start.resolve(strict=True).is_relative_to(source)
                or start.is_symlink() or not start.is_dir()):
            raise ValueError(f"unsafe or non-directory subtree: {subtree}")
        count = 0
        errors = []
        for directory, dirs, files in os.walk(start, followlinks=False, onerror=errors.append):
            directory = Path(directory)
            dirs[:] = sorted(d for d in dirs
                             if not (directory / d).is_symlink()
                             and (directory / d) != source / ".keeper"
                             and not (directory / d).is_relative_to(workspace))
            for name in sorted(files):
                path = directory / name
                if path == source / ".keeperignore":
                    continue
                meta = path.lstat()
                if not stat.S_ISREG(meta.st_mode):
                    continue
                count += 1
                if meta.st_size <= args.max_file_mib*MIB:
                    candidates[str(path.relative_to(source))] = meta.st_size
                if count >= args.scan_limit:
                    break
            if count >= args.scan_limit:
                break
        if errors:
            raise errors[0]
        scanned[subtree] = count
    selected = select(candidates, args.budget_mib*MIB, args.max_files)
    if not selected:
        raise ValueError("no eligible files selected")
    records = []
    started = time.monotonic()
    for relative, expected in selected:
        path = source / relative
        if not path.resolve(strict=True).is_relative_to(source):
            raise ValueError("source path escaped root")
        copies = [workspace / name / relative for name in ["sample-a", "sample-b"]]
        for copy in copies:
            copy.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(fd, "rb") as src, copies[0].open("xb") as a, copies[1].open("xb") as b:
            before = os.fstat(src.fileno())
            if not stat.S_ISREG(before.st_mode) or before.st_size != expected:
                raise ValueError("source changed since inventory")
            h = hashlib.sha256()
            for chunk in iter(lambda: src.read(MIB), b""):
                a.write(chunk); b.write(chunk); h.update(chunk)
            if fingerprint(before) != fingerprint(os.fstat(src.fileno())):
                raise ValueError("source changed during copying")
            for out in [a, b]:
                out.flush(); os.fsync(out.fileno())
        metas = [copy.stat() for copy in copies]
        if (any(m.st_dev != before.st_dev for m in metas)
                or len({before.st_ino, *(m.st_ino for m in metas)}) != 3):
            raise ValueError("copies are not independent files on the source filesystem")
        records.append({"path": relative, "size": expected, "sha256": h.hexdigest()})
    manifest.update(state="prepared", records=records, metadata_candidates_examined=scanned,
                    bytes_per_sample=sum(x["size"] for x in records),
                    files_per_sample=len(records), copy_seconds=time.monotonic()-started)
    if source_metadata(source) != manifest["source_metadata"]:
        raise ValueError("source catalog/policy changed during preparation")
    for directory, _, _ in os.walk(workspace, topdown=False):
        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
    save(workspace / MARKER, manifest)
    save(report / "preparation.json", manifest)
    print(f"Prepared {len(records):,} files / {manifest['bytes_per_sample']/MIB:,.1f} MiB per sample")
    print(f"Workspace: {workspace}\nReports: {report}")


def load_workspace(value):
    workspace = Path(value).resolve(strict=True)
    marker = workspace / MARKER
    if marker.is_symlink():
        raise ValueError("workspace marker must not be a symlink")
    manifest = json.loads(marker.read_text())
    if (manifest["version"] != 1 or manifest["state"] != "prepared"
            or manifest["workspace"] != str(workspace)
            or manifest["workspace_binding"] != fingerprint(workspace.stat())[:2]):
        raise ValueError("workspace identity/state mismatch")
    source = Path(manifest["source"])
    if manifest["source_binding"] != fingerprint(source.stat())[:2]:
        raise ValueError("source root identity changed")
    # Never follow replaced sample/scratch directories into originals.
    for name in ["sample-a", "sample-b", "test-tmp"]:
        path = workspace / name
        if path.is_symlink() or path.resolve(strict=True) != path or not path.is_dir():
            raise ValueError(f"unsafe workspace member: {path}")
    report = Path(manifest["report"])
    if report.resolve(strict=True) != report or report.is_relative_to(source):
        raise ValueError("unsafe report directory")
    return workspace, report, manifest


class Runner:
    def __init__(self, report, binary, timeout):
        self.report, self.binary, self.timeout = report, binary, timeout
        self.results = []

    def command(self, label, argv, expected=0, env=None, interrupt=None):
        start = time.monotonic()
        peak = 0
        sent = False
        with (self.report / (label+'.stdout')).open('xb') as out, (self.report / (label+'.stderr')).open('xb') as err:
            child = subprocess.Popen(list(map(str, argv)), cwd=REPO, env=env,
                                     stdout=out, stderr=err, start_new_session=True)
            try:
                while True:
                    try:
                        for line in Path(f'/proc/{child.pid}/status').read_text().splitlines():
                            if line.startswith('VmHWM:'):
                                peak = max(peak, int(line.split()[1]))
                    except FileNotFoundError:
                        pass
                    pid, status, usage = os.wait4(child.pid, os.WNOHANG)
                    if pid:
                        code = os.waitstatus_to_exitcode(status)
                        child.returncode = code
                        break
                    if interrupt and not sent:
                        work, sig = interrupt
                        if work.exists() and work.read_bytes().count(b'\n') >= 2:
                            child.send_signal(sig)
                            sent = True
                    if time.monotonic()-start > self.timeout:
                        raise TimeoutError(f'{label}: exceeded {self.timeout}s; see logs')
                    time.sleep(.001)
            except BaseException:
                # Own process group only; also terminate cargo's test children.
                if child.returncode is None:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    child.wait()
                raise
        result = {"label": label, "argv": list(map(str, argv)), "exit_code": code,
                  "expected_exit_code": expected, "passed": code == expected,
                  "seconds": time.monotonic()-start, "sampled_peak_rss_kib": peak,
                  "wait4_rss_upper_bound_kib": usage.ru_maxrss, "signal_sent": sent}
        if interrupt and not sent:
            result['passed'] = False
        self.results.append(result)
        save(self.report/'commands.json', self.results)
        print(f"{label}: {'PASS' if result['passed'] else 'FAIL'} ({result['seconds']:.2f}s)", flush=True)
        return (self.report/(label+'.stdout')).read_text(errors='backslashreplace')

    def keeper(self, label, args, expected=0, interrupt=None):
        return self.command(label, [self.binary, *args, '--color', 'never', '--progress', 'never'],
                            expected, interrupt=interrupt)


def timestamp_probe(root, trials=100):
    root.mkdir(exist_ok=False)
    path = root/'file'
    path.write_bytes(b'original')
    results = []
    fd = os.open(path, os.O_RDWR)
    try:
        for restore in [False, True]:
            for delay in [0, .0001, .001, .005]:
                collisions = 0
                for _ in range(trials):
                    os.pwrite(fd, b'original', 0)
                    before = os.fstat(fd)
                    if delay:
                        time.sleep(delay)
                    os.pwrite(fd, b'modified', 0)
                    if restore:
                        os.utime(fd, ns=(before.st_atime_ns, before.st_mtime_ns))
                    collisions += fingerprint(before) == fingerprint(os.fstat(fd))
                    if os.pread(fd, 8, 0) != b'modified':
                        raise ValueError('probe content assertion failed')
                results.append(dict(kind='file', restore_mtime=restore, delay=delay,
                                    trials=trials, unchanged_fingerprint=collisions))
    finally:
        os.close(fd)
    collisions = 0
    for i in range(trials):
        directory = root/f'dir-{i}'
        directory.mkdir()
        before = directory.stat()
        (directory/'new').write_bytes(b'new')
        collisions += fingerprint(before) == fingerprint(directory.stat())
    results.append(dict(kind='directory-addition', delay=0, trials=trials,
                        unchanged_fingerprint=collisions))
    # Determine whether a collision survives a flush and elapsed clock ticks.
    persist = []
    for i in range(3):
        fd = os.open(path, os.O_RDWR)
        try:
            for _ in range(100):
                os.pwrite(fd, b'original', 0); before = os.fstat(fd)
                os.pwrite(fd, b'modified', 0)
                if fingerprint(before) == fingerprint(os.fstat(fd)):
                    os.fsync(fd); time.sleep(.02)
                    persist.append(fingerprint(before) == fingerprint(os.fstat(fd)))
                    break
        finally:
            os.close(fd)
    precision = []
    fixed = 1700000000123456700
    for offset in [0, 1, 99, 100, 101, 999, 1000]:
        os.utime(path, ns=(fixed+offset, fixed+offset))
        actual = path.stat().st_mtime_ns
        precision.append({'requested_mtime_ns':fixed+offset, 'observed_mtime_ns':actual})
    return {"observations": results, "collisions_persisting_after_fsync_and_20ms": persist,
            "explicit_mtime_storage":precision,
            "note": "Empirical timing probes, not deterministic correctness tests or a snapshot guarantee."}


def probe(args):
    workspace, report, _ = load_workspace(args.workspace)
    name = 'probe-'+time.strftime('%Y%m%d-%H%M%S')
    result = {'target':timestamp_probe(workspace/'test-tmp'/name, args.trials)}
    if args.control:
        control = new_path(args.control)
        result['control_directory'] = str(control)
        result['control'] = timestamp_probe(control, args.trials)
    save(report/(name+'.json'), result)
    print(json.dumps(result, indent=2))


def run(args):
    workspace, report, manifest = load_workspace(args.workspace)
    if (report/'RUNNING.json').exists():
        raise ValueError('workspace already used; prepare a new workspace for another qualification')
    a, b = workspace/'sample-a', workspace/'sample-b'
    for tree in [a, b]:
        if (tree/'.keeper').exists() or (tree/'qualification-edge-cases').exists():
            raise ValueError('samples are not pristine; prepare new copies')
    source = Path(manifest['source'])
    original = source_metadata(source)
    if original != manifest['source_metadata']:
        raise ValueError('source catalog/policy changed since preparation')
    save(report/'RUNNING.json', {'started': time.time(), 'workspace': str(workspace)})
    runner = Runner(report, Path(args.binary).resolve(), args.timeout)
    # Verify copies before modifying them, including independence from originals.
    for record in manifest['records']:
        for tree in [a, b]:
            path = tree/record['path']
            if (path.is_symlink() or not path.resolve(strict=True).is_relative_to(tree)
                    or digest(path) != record['sha256']):
                raise ValueError('prepared copy was modified or replaced')
    runner.command('environment', ['sh', '-c',
        'date -Is; uname -a; rustc --version; git rev-parse HEAD; git status --short; findmnt -T "$1"; df -h "$1"; lsblk -d -o NAME,MODEL,ROTA,TRAN,SIZE',
        'qualify', str(workspace)])
    runner.command('release-build', ['cargo', 'build', '--release', '--locked'])
    if not runner.results[-1]['passed']:
        raise ValueError('release build failed; refusing to test a potentially stale binary')
    paths = subprocess.check_output(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=REPO)
    sources = {}
    for raw in paths.split(b'\0'):
        if raw:
            relative = os.fsdecode(raw)
            path = REPO/relative
            sources[relative] = (str(path.readlink()) if path.is_symlink()
                                 else digest(path) if path.is_file() else None)
    save(report/'provenance.json', {'source_sha256_or_symlink_target':sources,
                                   'binary_sha256':digest(runner.binary)})
    if not args.skip_suite:
        env = dict(os.environ, TMPDIR=str(workspace/'test-tmp'))
        runner.command('cargo-test', ['cargo', 'test', '--locked', '--no-fail-fast'], env=env)
    limitations = []
    for tree in [a, b]:
        edge = tree/'qualification-edge-cases'
        edge.mkdir()
        (edge/'original').write_bytes(b'duplicate bytes\n')
        shutil.copyfile(edge/'original', edge/'copy')
        try:
            os.link(edge/'original', edge/'hardlink')
        except OSError as exc:
            limitations.append({'probe':'hardlink', 'tree':str(tree), 'error':str(exc)})
        for name in ['spaces and unicode-zażółć.txt', 'tab\tand-newline\n.txt', b'non-utf8-\xff']:
            try:
                fd = os.open(os.fsencode(edge)+b'/'+os.fsencode(name), os.O_WRONLY|os.O_CREAT|os.O_EXCL, 0o600)
                try:
                    os.write(fd, b'filename probe\n')
                finally:
                    os.close(fd)
            except OSError as exc:
                limitations.append({'probe':repr(name), 'tree':str(tree), 'error':str(exc)})
    save(report/'capability-limitations.json', limitations)
    for name, tree in [('a',a), ('b',b)]:
        runner.keeper('preflight-'+name, ['preflight',tree])
        runner.keeper('init-'+name, ['init','--resume',tree])
        runner.keeper('catalog-check-'+name, ['catalog','check',tree])
        if not (tree/'.keeper/catalog').is_dir():
            raise ValueError('initialization failed; inspect logs')
    baselines = [snapshot(tree/'.keeper/catalog') for tree in [a,b]]
    runner.keeper('recorded-compare', ['compare',a,b])
    runner.keeper('live-compare', ['compare','--live',a,b])
    for name, tree in [('a',a), ('b',b)]:
        runner.keeper('fresh-status-'+name, ['status',tree])
        runner.keeper('checkpointed-status-'+name, ['status','--resume',tree])
    # Deliberate changes are always restored, even if a check raises.
    target = a/'qualification-edge-cases/copy'
    removed = a/'qualification-edge-cases/spaces and unicode-zażółć.txt'
    added = a/'qualification-edge-cases/added'
    old, meta = target.read_bytes(), target.stat()
    removed_data = removed.read_bytes() if removed.exists() else None
    try:
        target.write_bytes(bytes([old[0]^1])+old[1:])
        os.utime(target, ns=(meta.st_atime_ns,meta.st_mtime_ns))
        if removed_data is not None:
            removed.unlink()
        added.write_bytes(b'added\n')
        text = runner.keeper('deliberate-drift', ['status',a], 1)
        markers = ['CHANGED\t','ADDED\t'] + (['MISSING\t'] if removed_data is not None else [])
        if not all(marker in text for marker in markers):
            raise ValueError('drift report missing expected categories')
        runner.keeper('live-drift', ['compare','--live',a,b], 1)
        runner.keeper('recorded-still-agrees', ['compare',a,b])
        runner.keeper('reinit-does-not-accept', ['init',a])
    finally:
        target.write_bytes(old)
        os.utime(target, ns=(meta.st_atime_ns,meta.st_mtime_ns))
        if removed_data is not None:
            removed.write_bytes(removed_data)
        added.unlink(missing_ok=True)
    for name, tree, baseline in zip(['a','b'], [a,b], baselines):
        if snapshot(tree/'.keeper/catalog') != baseline:
            raise ValueError('sample baseline changed')
        runner.keeper('export-'+name, ['catalog','export',tree,report/('catalog-'+name)])
        if snapshot(report/('catalog-'+name)) != baseline:
            raise ValueError('export metadata differs')
        runner.keeper('final-status-'+name, ['status',tree])
    runner.keeper('offline-compare', ['compare','--catalogs',report/'catalog-a',report/'catalog-b'])
    # Dedicated sparse-file corpus makes interruption independent of sample size.
    lab = workspace/'cancellation'
    lab.mkdir()
    (lab/'a').write_bytes(b'original')
    with (lab/'z-large').open('xb') as stream:
        stream.truncate(2*1024*MIB)
    runner.keeper('init-cancel', ['init','--resume',lab], 2,
                  interrupt=(lab/'.keeper/init.work',signal.SIGTERM))
    resumed = runner.keeper('init-resume', ['init','--resume',lab])
    runner.keeper('status-kill', ['status','--resume',lab], -signal.SIGKILL,
                  interrupt=(lab/'.keeper/status.work',signal.SIGKILL))
    meta = (lab/'a').stat()
    (lab/'a').write_bytes(b'modified')
    os.utime(lab/'a', ns=(meta.st_atime_ns,meta.st_mtime_ns))
    try:
        runner.keeper('status-resume-changed', ['status','--resume',lab], 1)
    finally:
        (lab/'a').write_bytes(b'original')
    runner.keeper('lab-restored-status', ['status',lab])
    # Known restricted roots must never claim a reused physical read.
    env_text = (report/'environment.stdout').read_text()
    restricted = bool(re.search(r'\b(ntfs3?|vfat|exfat|fuse\S*)\b',env_text))
    if restricted and '0 file(s) reused' not in resumed:
        raise ValueError('restricted filesystem reused checkpoint bytes')
    runner.keeper('warm-status', ['status',a])
    if args.evict_sample_cache:
        for directory, dirs, files in os.walk(a):
            if Path(directory) == a:
                dirs[:] = [d for d in dirs if d != '.keeper']
            for name in files:
                fd = os.open(Path(directory)/name, os.O_RDONLY|os.O_NOFOLLOW)
                try:
                    os.fsync(fd)
                    os.posix_fadvise(fd,0,0,os.POSIX_FADV_DONTNEED)
                finally:
                    os.close(fd)
        runner.keeper('advisory-evicted-status', ['status',a])
        runner.keeper('rewarmed-status', ['status',a])
    save(report/'timestamp-probe.json', timestamp_probe(workspace/'test-tmp/timestamp-probe'))
    if source_metadata(source) != original:
        raise ValueError('original catalog/policy changed')
    failed = [r['label'] for r in runner.results if not r['passed']]
    summary = {'result':'failed' if failed or limitations else 'incomplete' if args.skip_suite else 'sample-checks-passed',
               'failed_commands':failed, 'capability_limitations':limitations,
               'source_catalog_and_policy_unchanged':True, 'samples_restored':True,
               'cache_note':'Recent copies/warm cache except optional advisory per-file eviction; no global cache drop.',
               'memory_note':'/proc VmHWM is sampled; wait4 RSS includes inherited launcher high-water memory.',
               'limits':['No power-loss test','No whole-drive integrity or memory bound',
                         'Concurrent writers are unsupported, including throughout resumed sessions',
                         'Timing probes characterize metadata; no collision-free guarantee']}
    save(report/'summary.json',summary)
    (report/'RESULTS.md').write_text('# Sample qualification\n\n```json\n'+json.dumps(summary,indent=2)+'\n```\n\nSee commands.json, individual stdout/stderr logs, preparation.json and timestamp-probe.json.\n')
    print(f"Result: {summary['result']}; reports: {report}")
    return 1 if failed or limitations else 0


def positive(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError('must be positive')
    return number


def parser():
    cli = argparse.ArgumentParser(description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog='Syntax:\n  prepare --source ROOT --workspace NEW_DIR --report NEW_DIR [--subtree PATH]...\n  run --workspace DIR [--evict-sample-cache]\n  probe --workspace DIR [--control NEW_DIR]\n  help [prepare|run|probe]')
    commands = cli.add_subparsers(dest='command', required=True)
    p = commands.add_parser('prepare', help='copy bounded samples; originals are read only')
    p.add_argument('--source',required=True)
    p.add_argument('--workspace',required=True,help='new directory on source filesystem')
    p.add_argument('--report',required=True,help='new directory outside source; prefer another device')
    p.add_argument('--subtree',action='append',help='relative directory to sample; repeatable (default: whole source)')
    p.add_argument('--budget-mib',type=positive,default=1024)
    p.add_argument('--max-file-mib',type=positive,default=512)
    p.add_argument('--max-files',type=positive,default=1200)
    p.add_argument('--scan-limit',type=positive,default=12000,help='maximum regular files examined per subtree')
    p.set_defaults(action=prepare)
    p = commands.add_parser('run', help='test prepared disposable samples; saves logs and exports')
    p.add_argument('--workspace',required=True)
    p.add_argument('--binary',default=str(REPO/'target/release/keeper'))
    p.add_argument('--timeout',type=positive,default=600,help='per-command timeout in seconds')
    p.add_argument('--skip-suite',action='store_true',help='workflow smoke test only; result is incomplete')
    p.add_argument('--evict-sample-cache',action='store_true',help='advisory cache eviction on sample files only')
    p.set_defaults(action=run)
    p = commands.add_parser('probe',help='record timestamp collisions in workspace scratch files')
    p.add_argument('--workspace',required=True)
    p.add_argument('--trials',type=positive,default=100)
    p.add_argument('--control',help='optional NEW scratch directory for a filesystem control comparison')
    p.set_defaults(action=probe)
    p = commands.add_parser('help',help='show top-level or focused command help')
    p.add_argument('topic',nargs='?',choices=['prepare','run','probe'])
    return cli


def main(argv=None):
    cli = parser()
    args = cli.parse_args(argv)
    if args.command == 'help':
        cli.parse_args(([args.topic] if args.topic else [])+['--help'])
    try:
        return args.action(args) or 0
    except KeyboardInterrupt:
        print('qualification interrupted; partial artifacts retained for inspection', file=sys.stderr)
        return 130
    except (OSError, ValueError, KeyError, TimeoutError, subprocess.SubprocessError) as exc:
        print(f'qualification error: {exc}; partial artifacts retained for inspection',file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
