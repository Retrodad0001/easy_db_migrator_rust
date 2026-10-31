FROM rust:slim AS profile
WORKDIR /build
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates build-essential \
       pkg-config libssl-dev linux-perf \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install --locked hyperfine --version 1.20.0 \
    && cargo install --locked flamegraph --version 0.6.14
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY profiling ./profiling
COPY tests ./tests
ARG CARGO_PROFILE_RELEASE_DEBUG
ENV CARGO_PROFILE_RELEASE_DEBUG=${CARGO_PROFILE_RELEASE_DEBUG} CARGO_PROFILE_RELEASE_STRIP=none
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/build/target-profile,sharing=locked \
    set -eu; \
    cargo build --release -p easy_db_migrator_rust --example profile_migrations \
        --features postgres,mssql --target-dir /build/target-profile; \
    cp /build/target-profile/release/examples/profile_migrations /build/profile-migrations; \
    cargo build --release -p easy_db_migrator_rust --example profile_migrations \
        --features postgres,mssql,dhat-heap --target-dir /build/target-profile; \
    cp /build/target-profile/release/examples/profile_migrations /build/profile-migrations-dhat
COPY --chmod=755 <<'EOF' /usr/local/bin/profile-migrator
#!/bin/sh
set -eu

scripts=/build/scripts
step=""
# the versions and scripts, a wait per database, and 3 tools for each of the 12 loads
steps=39
step_number=0

begin() {
    step=$1
    step_number=$((step_number + 1))
    echo "  [$step_number/$steps, $(((step_number - 1) * 100 / steps))% done] BEGIN $1"
}

end() {
    echo "  END $1 - passed"
    step=""
}

report_a_failed_step() {
    if [ -n "$step" ]; then
        echo "  END $step - FAILED"
    fi
}

trap report_a_failed_step EXIT

connection_string_of() {
    case "$1" in
        postgres) echo "$POSTGRES_URL" ;;
        mssql) echo "$MSSQL_URL" ;;
        *)
            echo "no database named $1" >&2
            return 1
            ;;
    esac
}

run() {
    connection_string=$(connection_string_of "$1")
    /build/profile-migrations "$1" "$2" "$connection_string" "$scripts/$3"
}

# the sequence part of a script name has 3 digits, so the scripts move to the next
# day after 500
write_the_table_scripts() {
    directory="$scripts/$1"
    mkdir -p "$directory"
    i=1
    while [ "$i" -le "$1" ]; do
        day=$(((i - 1) / 500 + 1))
        sequence=$(((i - 1) % 500 + 1))
        number=$(printf '%04d' "$i")
        filename=$(printf '2026010%d_%03d_create_table_%s.sql' "$day" "$sequence" "$number")
        printf 'CREATE TABLE table_%s (id INTEGER PRIMARY KEY, title VARCHAR(40) NOT NULL);\n' "$number" \
            > "$directory/$filename"
        i=$((i + 1))
    done
}

write_the_big_script() {
    mkdir -p "$scripts/big"
    {
        echo 'CREATE TABLE big_table (id INTEGER PRIMARY KEY, title VARCHAR(40) NOT NULL);'
        i=1
        while [ "$i" -le 10000 ]; do
            printf "INSERT INTO big_table (id, title) VALUES (%d, 'title %d');\n" "$i" "$i"
            i=$((i + 1))
        done
    } > "$scripts/big/20260101_001_insert_rows.sql"
}

wait_until_answering() {
    tries=0
    until run "$1" delete 100 > /dev/null 2>&1; do
        tries=$((tries + 1))
        if [ "$tries" -ge 180 ]; then
            echo "$1 did not answer within 180 seconds" >&2
            return 1
        fi
        sleep 1
    done
}

prepare_the_database() {
    run "$1" delete "$3"
    if [ "$2" = applied ]; then
        run "$1" apply "$3"
    fi
}

measure_the_time() {
    name="$1-$2-$3"
    begin "hyperfine: $name"
    mkdir -p "/results/$name"
    prepare_the_database "$1" "$2" "$3"
    if [ "$3" = 100 ]; then
        warmup=3
        runs=20
    else
        warmup=1
        runs=5
    fi
    if [ "$2" = new ]; then
        hyperfine --warmup "$warmup" --runs "$runs" \
            --prepare "profile-migrator run $1 delete $3" \
            --export-json "/results/$name/hyperfine.json" \
            "profile-migrator run $1 apply $3"
    else
        hyperfine --warmup "$warmup" --runs "$runs" \
            --export-json "/results/$name/hyperfine.json" \
            "profile-migrator run $1 apply $3"
    fi
    date -Iseconds > "/results/$name/timing.time"
    end "hyperfine: $name"
}

draw_the_flamegraph() {
    name="$1-$2-$3"
    begin "cargo-flamegraph: $name"
    prepare_the_database "$1" "$2" "$3"
    mkdir -p "/tmp/flamegraph-$name"
    cd "/tmp/flamegraph-$name"
    connection_string=$(connection_string_of "$1")
    if ! flamegraph --cmd "record -F 4999 --call-graph dwarf,64000 -g -m 64M" \
        --post-process "tee /results/$name/cpu.folded" \
        -o "/results/$name/flamegraph.svg" \
        -- /build/profile-migrations "$1" apply "$connection_string" "$scripts/$3" > flamegraph.log 2>&1; then
        cat flamegraph.log >&2
        return 1
    fi
    if grep -qi "lost" flamegraph.log; then
        grep -i "lost" flamegraph.log >&2
        echo "perf lost samples, so the flamegraph of $name is not complete" >&2
        return 1
    fi
    sed -n 's/.*Captured and wrote .* (\([0-9]*\) samples).*/\1/p' flamegraph.log > "/results/$name/cpu.samples"
    echo "  perf recorded $(cat "/results/$name/cpu.samples") samples"
    date -Iseconds > "/results/$name/cpu.time"
    end "cargo-flamegraph: $name"
}

record_the_heap() {
    name="$1-$2-$3"
    begin "dhat: $name"
    prepare_the_database "$1" "$2" "$3"
    mkdir -p "/tmp/dhat-$name"
    cd "/tmp/dhat-$name"
    connection_string=$(connection_string_of "$1")
    if ! /build/profile-migrations-dhat "$1" apply "$connection_string" "$scripts/$3" > dhat.log 2>&1; then
        cat dhat.log >&2
        return 1
    fi
    cp dhat-heap.json "/results/$name/dhat-heap.json"
    date -Iseconds > "/results/$name/memory.time"
    end "dhat: $name"
}

case "${1:-}" in
    run) run "$2" "$3" "$4" ;;
    all)
        begin "writing the tool versions and the scripts"
        write_the_table_scripts 100
        write_the_table_scripts 1000
        write_the_big_script
        {
            hyperfine --version
            flamegraph --version
            perf --version
            rustc --version
        } > /results/versions.txt
        end "writing the tool versions and the scripts"
        for database in postgres mssql; do
            begin "waiting until $database answers"
            wait_until_answering "$database"
            end "waiting until $database answers"
            for script_set in 100 1000 big; do
                for kind in new applied; do
                    measure_the_time "$database" "$kind" "$script_set"
                    draw_the_flamegraph "$database" "$kind" "$script_set"
                    record_the_heap "$database" "$kind" "$script_set"
                done
            done
        done
        ;;
    *)
        echo "use profile-migrator all, or profile-migrator run <postgres|mssql> <delete|apply> <100|1000|big>" >&2
        exit 2
        ;;
esac
EOF
