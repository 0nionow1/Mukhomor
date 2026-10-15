package core

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"unicode/utf8"

	"golang.org/x/sys/unix"
)

const MaxFrame = 4 << 20
const MaxImport = 512 << 10
const MaxSettings = 2 << 20

type Object = map[string]any

func str(v any) string { s, _ := v.(string); return s }
func obj(v any) Object {
	m, _ := v.(map[string]any)
	if m == nil {
		return Object{}
	}
	return m
}
func array(v any) []any { a, _ := v.([]any); return a }
func clone(v any) any   { b, _ := json.Marshal(v); var r any; _ = json.Unmarshal(b, &r); return r }
func randomID() string {
	b := make([]byte, 16)
	if _, err := rand.Read(b); err != nil {
		panic(err)
	}
	return hex.EncodeToString(b)
}

var validID = regexp.MustCompile(`^[a-f0-9]{32}$`)

func validName(s string) bool {
	return strings.TrimSpace(s) != "" && utf8.RuneCountInString(s) <= 80 && !strings.ContainsAny(s, "\x00\r\n\t")
}

// Store keeps one atomic document: profile credentials, selection and settings
// commit together. The lock prevents a second controller from losing updates.
type Store struct {
	dir  *os.Root
	lock *os.File
	path string
}

func OpenStore(path string) (*Store, error) {
	path, err := filepath.Abs(path)
	if err != nil {
		return nil, err
	}
	if err = os.MkdirAll(path, 0700); err != nil {
		return nil, err
	}
	fi, err := os.Lstat(path)
	if err != nil {
		return nil, err
	}
	if !fi.IsDir() || fi.Mode()&os.ModeSymlink != 0 || fi.Mode().Perm()&0077 != 0 {
		return nil, errors.New("Private data directory must have mode 0700")
	}
	var st unix.Stat_t
	if err = unix.Lstat(path, &st); err != nil || st.Uid != uint32(os.Getuid()) {
		return nil, errors.New("Private data directory has a different owner")
	}
	dir, err := os.OpenRoot(path)
	if err != nil {
		return nil, err
	}
	fd, err := unix.Open(filepath.Join(path, ".controller.lock"), unix.O_CREAT|unix.O_RDWR|unix.O_CLOEXEC|unix.O_NOFOLLOW, 0600)
	if err != nil {
		dir.Close()
		return nil, err
	}
	lock := os.NewFile(uintptr(fd), ".controller.lock")
	if err = unix.Flock(fd, unix.LOCK_EX|unix.LOCK_NB); err != nil {
		lock.Close()
		dir.Close()
		return nil, errors.New("Mukhomor is already running for this data directory")
	}
	return &Store{dir, lock, path}, nil
}

func (s *Store) Close() { _ = s.lock.Close(); _ = s.dir.Close() }
func (s *Store) Read() (Object, error) {
	fi, err := s.dir.Lstat("library.json")
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	if !fi.Mode().IsRegular() || fi.Mode().Perm()&0077 != 0 {
		return nil, errors.New("Unsafe profile library permissions")
	}
	f, err := s.dir.Open("library.json")
	if err != nil {
		return nil, err
	}
	defer f.Close()
	b, err := io.ReadAll(io.LimitReader(f, 32<<20+1))
	if err != nil || len(b) > 32<<20 {
		return nil, errors.New("Invalid profile library size")
	}
	var data Object
	if err = json.Unmarshal(b, &data); err != nil {
		return nil, errors.New("Profile library is damaged; restore its backup before continuing")
	}
	return data, nil
}
func (s *Store) Write(data Object) error {
	b, err := json.Marshal(data)
	if err != nil || len(b) > 32<<20 {
		return errors.New("Profile library is too large")
	}
	name := ".library-" + randomID() + ".tmp"
	f, err := s.dir.OpenFile(name, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
	defer s.dir.Remove(name)
	_, err = f.Write(b)
	if err == nil {
		err = f.Sync()
	}
	closeErr := f.Close()
	if err == nil {
		err = closeErr
	}
	if err != nil {
		return err
	}
	if err = s.dir.Rename(name, "library.json"); err != nil {
		return err
	}
	dir, err := s.dir.Open(".")
	if err != nil {
		return err
	}
	defer dir.Close()
	return dir.Sync()
}
