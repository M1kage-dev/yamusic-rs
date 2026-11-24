# Run from any directory: ruby tests/ci_contract.rb
require 'digest'
require 'fileutils'
require 'open3'
require 'tmpdir'
require 'yaml'

ROOT = File.expand_path('..', __dir__)
JOBS = YAML.safe_load(File.read(File.join(ROOT, '.github/workflows/build.yml'))).fetch('jobs')

def check(condition, message)
  abort message unless condition
end

release = JOBS.fetch('release')
check(Array(release['needs']) == ['macos'], 'release must depend only on macos')
check(JOBS.keys.sort == %w[linux macos release windows], 'unexpected job (ARM Linux/APK must stay out of CI)')
check(release['if'] == "startsWith(github.ref, 'refs/tags/v')", 'release must be restricted to v* tags')

{ 'windows' => 'windows-latest', 'linux' => 'ubuntu-22.04' }.each do |job, runner|
  config = JOBS.fetch(job)
  check(config['runs-on'] == runner, "#{job}: expected an x64 runner")
  steps = config.fetch('steps')
  commands = steps.map { |step| step['run'] }.compact.reject { |run| run.start_with?('sudo apt-get ') }
  check(commands.sort == ['cargo clippy --release --all-targets -- -D warnings', 'cargo test --release'],
        "#{job}: expected only clippy and tests after setup")
end

uploads = JOBS.flat_map do |job, config|
  config.fetch('steps').select { |step| step['uses'].to_s.start_with?('actions/upload-artifact@') }
        .map { |step| [job, step] }
end
check(uploads.size == 1 && uploads.first.first == 'macos', 'macos must produce the only artifact')
macos = JOBS.fetch('macos')
check(macos.fetch('steps').any? { |step| step['run'] == 'tools/macos-app.sh --universal --skip-build --dmg' },
      'macOS must build a universal DMG')
upload = uploads.first.last
download = release.fetch('steps').find { |step| step['uses'].to_s.start_with?('actions/download-artifact@') }
check(upload.fetch('with').values_at('path', 'archive', 'if-no-files-found') ==
      ['target/macos/yamusic-rs-*-macos.dmg', false, 'error'], 'upload must require a single, unzipped DMG')
check(download.fetch('with').values_at('artifact-ids', 'path') == ['${{ needs.macos.outputs.dmg-id }}', 'dist'] &&
      macos.fetch('outputs').fetch('dmg-id') == "${{ steps.#{upload.fetch('id')}.outputs.artifact-id }}",
      'release must download the macOS artifact by ID')

scripts = release.fetch('steps').map { |step| step['run'] }.compact
notes = scripts.find { |script| script.include?('CHANGELOG.md') }
publish = scripts.find { |script| script.include?('gh release create') }
check(notes && publish, 'release must extract changelog notes and publish a DMG')

Dir.mktmpdir('ci-contract-') do |dir|
  Dir.chdir(dir) do
    # Fixture tests the full tag (including +metadata) and section boundaries.
    File.write('CHANGELOG.md', <<~CHANGELOG)
      ## [Unreleased]
      Unreleased text
      ## [1.2.3+upstream.1.1.6] — fixture
      Release text
      ## [1.2.2]
      Previous text
    CHANGELOG
    File.write('gh', <<~SH)
      #!/bin/bash -eu
      test "$#" -eq 8
      test "$1 $2" = 'release create'
      test "$3" = "$GITHUB_REF_NAME"
      test "$4" = 'dist/yamusic-rs-1.2.3-macos.dmg'
      test "$5" = '--title'
      test "$7 $8" = '--notes-file full.md'
      cp "$8" published.md
    SH
    File.chmod(0o755, 'gh')
    env = { 'GITHUB_REF_NAME' => 'v1.2.3+upstream.1.1.6', 'PATH' => "#{dir}:#{ENV.fetch('PATH')}" }
    run = ->(script, vars = env) { Open3.capture2e(vars, 'bash', '-eo', 'pipefail', '-c', script) }
    output, status = run.call(notes, env.merge('GITHUB_REF_NAME' => 'v9.9.9'))
    check(!status.success? && output.include?('## [9.9.9]'), 'missing changelog section must fail')
    output, status = run.call(notes)
    check(status.success?, "notes step failed: #{output}")

    FileUtils.mkdir('dist')
    dmg = 'yamusic-rs-1.2.3-macos.dmg'
    check_rejected = ->(reason) do
      log, result = run.call(publish)
      check(!result.success? && log.include?('::error::') && !File.exist?('published.md'),
            "#{reason} must block publication before invoking gh: #{log}")
    end
    check_rejected.call('missing DMG')
    File.binwrite("dist/#{dmg}", "fixture\x00\xFF".b)
    extra = 'dist/yamusic-rs-1.2.4-macos.dmg'
    FileUtils.cp("dist/#{dmg}", extra)
    check_rejected.call('extra DMG')
    File.delete(extra)
    File.rename("dist/#{dmg}", 'dist/other.zip')
    check_rejected.call('non-DMG artifact')
    File.rename('dist/other.zip', "dist/#{dmg}")
    output, status = run.call(publish)
    check(status.success? && File.exist?('published.md'), "publishing a single DMG failed: #{output}")
    published = File.read('published.md')
    check(published.include?('Release text') && !published.match?(/Unreleased text|Previous text/),
          'published notes must contain only the requested changelog section')
    digest = Digest::SHA256.file("dist/#{dmg}").hexdigest
    check(published.lines.grep(/^\| `/).map(&:strip) == ["| `#{dmg}` | `#{digest}` |"],
          'published notes must include exactly one checksum for the DMG')

    changelog = File.read(File.join(ROOT, 'CHANGELOG.md'))
    latest = changelog.split(/^## \[/).find { |section| section.match?(/\A\d/) }
    version = latest.split(']', 2).first
    File.write('CHANGELOG.md', changelog)
    output, status = run.call(notes, env.merge('GITHUB_REF_NAME' => "v#{version}"))
    check(status.success? && File.read('notes.md') == latest.lines.drop(1).join,
          "latest CHANGELOG section v#{version} was not extracted verbatim: #{output}")
    puts "CHANGELOG: v#{version} OK"
  end
end
puts 'CI contract: OK'
