import os
import dotenv
from pathlib import Path
from datetime import date
from urllib.parse import quote

from django.core.exceptions import ImproperlyConfigured

from app.services import get_log_dir

dotenv.load_dotenv()

# Build paths inside the project like this: BASE_DIR / 'subdir'.
BASE_DIR = Path(__file__).resolve().parent.parent


def require_env(name: str) -> str:
    """Read a required setting, or fail saying which one is missing.

    String-concatenating a missing variable raises TypeError somewhere deep in
    settings import, which tells you nothing about what to fix.
    """
    value = os.getenv(name)
    if not value:
        raise ImproperlyConfigured(
            f'{name} is not set. Copy .env.example to .env and fill it in.'
        )
    return value


def env_list(name: str, default: str = '') -> list[str]:
    return [item.strip() for item in os.getenv(name, default).split(',') if item.strip()]


# Quick-start development settings - unsuitable for production
# See https://docs.djangoproject.com/en/3.2/howto/deployment/checklist/

# SECURITY WARNING: keep the secret key used in production secret!
DEBUG = bool(os.getenv('DEBUG') == 'True')
SECRET_KEY = require_env('SECRET_KEY')
MAIN_HOST = os.getenv('MAIN_HOST', '')

# A wildcard here lets an attacker set any Host header, which poisons every
# absolute URL Django builds. Pin it to the domain actually being served.
ALLOWED_HOSTS = env_list('ALLOWED_HOSTS')
if not ALLOWED_HOSTS and MAIN_HOST:
    # Django matches ALLOWED_HOSTS against the host alone, so an entry
    # carrying a port can never match.
    _host = MAIN_HOST.split('//')[-1].split('/')[0].split(':')[0]
    ALLOWED_HOSTS = [_host]
    if not _host.replace('.', '').isdigit():
        ALLOWED_HOSTS.append(f'www.{_host}')
if DEBUG:
    ALLOWED_HOSTS += ['localhost', '127.0.0.1', '[::1]', 'testserver', '.localtest.me']


# Application definition

INSTALLED_APPS = [
    'django.contrib.admin',
    'django.contrib.auth',
    'django.contrib.contenttypes',
    'django.contrib.sessions',
    'django.contrib.messages',
    'django.contrib.staticfiles',
]

# third party apps
INSTALLED_APPS += [
    'rest_framework',
    'corsheaders',
]

# project apps
INSTALLED_APPS += [
    'app',
]

MIDDLEWARE = [
    'django.middleware.security.SecurityMiddleware',
    # Serves STATIC_ROOT straight from gunicorn. django.conf.urls.static is a
    # no-op once DEBUG is off, and there is no nginx in front, so without this
    # every stylesheet 404s in production.
    'whitenoise.middleware.WhiteNoiseMiddleware',
    'django.contrib.sessions.middleware.SessionMiddleware',
    'corsheaders.middleware.CorsMiddleware',
    'django.middleware.common.CommonMiddleware',
    'django.middleware.csrf.CsrfViewMiddleware',
    'django.contrib.auth.middleware.AuthenticationMiddleware',
    'django.contrib.messages.middleware.MessageMiddleware',
    'django.middleware.clickjacking.XFrameOptionsMiddleware',
]

ROOT_URLCONF = 'config.urls'

TEMPLATES = [
    {
        'BACKEND': 'django.template.backends.django.DjangoTemplates',
        'DIRS': [os.path.join(BASE_DIR / 'templates')],
        'APP_DIRS': True,
        'OPTIONS': {
            'context_processors': [
                'django.template.context_processors.debug',
                'django.template.context_processors.request',
                'django.contrib.auth.context_processors.auth',
                'django.contrib.messages.context_processors.messages',
            ],
        },
    },
]

WSGI_APPLICATION = 'config.wsgi.application'


# Database
# https://docs.djangoproject.com/en/3.2/ref/settings/#databases

DATABASES = {
    'default': {
        'ENGINE': 'django.db.backends.' + require_env('DB_ENGINE'),
        'NAME': require_env('POSTGRES_DB'),
        'USER': require_env('POSTGRES_USER'),
        'PASSWORD': require_env('POSTGRES_PASSWORD'),
        'HOST': require_env('POSTGRES_HOST'),
        'PORT': os.getenv('POSTGRES_PORT', '5432'),
    }
}


# Password validation
# https://docs.djangoproject.com/en/3.2/ref/settings/#auth-password-validators

AUTH_PASSWORD_VALIDATORS = [
    {
        'NAME': 'django.contrib.auth.password_validation.UserAttributeSimilarityValidator',
    },
    {
        'NAME': 'django.contrib.auth.password_validation.MinimumLengthValidator',
    },
    {
        'NAME': 'django.contrib.auth.password_validation.CommonPasswordValidator',
    },
    {
        'NAME': 'django.contrib.auth.password_validation.NumericPasswordValidator',
    },
]


# Internationalization
# https://docs.djangoproject.com/en/3.2/topics/i18n/

LANGUAGE_CODE = 'en-us'
TIME_ZONE = 'Asia/Tashkent'
USE_I18N = True
USE_L10N = True
USE_TZ = True


# Static files (CSS, JavaScript, Images)
# https://docs.djangoproject.com/en/3.2/howto/static-files/

STATIC_URL = '/static/'
STATIC_ROOT = os.path.join(BASE_DIR, 'staticfiles')
STATICFILES_DIRS = [os.path.join(BASE_DIR, 'static')]

STORAGES = {
    'default': {
        'BACKEND': 'django.core.files.storage.FileSystemStorage',
    },
    'staticfiles': {
        # Hashed filenames + gzip/brotli, so collected assets can be cached
        # forever and served compressed by WhiteNoise.
        'BACKEND': 'whitenoise.storage.CompressedManifestStaticFilesStorage',
    },
}

MEDIA_URL = '/media/'
MEDIA_ROOT = os.path.join(BASE_DIR, 'media/')

# Default primary key field type
# https://docs.djangoproject.com/en/3.2/ref/settings/#default-auto-field

DEFAULT_AUTO_FIELD = 'django.db.models.BigAutoField'

# Loggers
# None means no writable directory was found — fall back to console-only
# logging rather than letting dictConfig raise and take the process down.
LOG_DIR = get_log_dir(base_dir=BASE_DIR)
_LOG_HANDLERS = ['console', 'common'] if LOG_DIR else ['console']

LOGGING = {
    'version': 1,
    'disable_existing_loggers': False,
    'formatters': {
        'verbose': {
            'format': '{levelname} - {asctime} ===>| {message}',
            'style': '{',
        },
    },
    "handlers": {
        'console': {
            'level': 'DEBUG',
            'class': 'logging.StreamHandler',
            'formatter': 'verbose',
        },
    },
    'loggers': {
        # 'django': {
        #     'handlers': ['console'],
        #     'level': 'DEBUG',
        #     'propagate': True,
        # },
        'Common': {
            'handlers': _LOG_HANDLERS,
            'propagate': True,
            'level': 'DEBUG'
        }
    }
}

if LOG_DIR:
    LOGGING['handlers']['common'] = {
        'level': 'DEBUG',
        'class': 'logging.handlers.TimedRotatingFileHandler',
        'filename': os.path.join(LOG_DIR, 'log-' + str(date.today()) + '.log'),
        'when': 'midnight',
        'interval': 1,
        'backupCount': 0,
        'formatter': 'verbose',
    }

CORS_ALLOW_METHODS = [
    'DELETE',
    'GET',
    'OPTIONS',
    'PATCH',
    'POST',
    'PUT',
]

CSRF_TRUSTED_ORIGINS = env_list('CORS_ALLOWED_ORIGINS')
# The dashboard is same-origin and there is no public API, so there is nothing
# here that a wildcard would enable except cross-origin reads of user pages.
CORS_ALLOWED_ORIGINS = CSRF_TRUSTED_ORIGINS
CORS_ALLOW_ALL_ORIGINS = DEBUG

# Redis conf
# Redis holds the auth index the Rust gateway reads on the hot path, so it is
# password-protected and never published to the host (see compose.yml). The
# empty password default keeps a bare local `redis-server` usable in dev.
REDIS_HOST = os.getenv('REDIS_HOST', 'redis')
REDIS_PORT = os.getenv('REDIS_PORT', '6379')
REDIS_PASSWORD = os.getenv('REDIS_PASSWORD', '')

_redis_auth = f':{quote(REDIS_PASSWORD, safe="")}@' if REDIS_PASSWORD else ''
REDIS_URL = f'redis://{_redis_auth}{REDIS_HOST}:{REDIS_PORT}'

# db 0 is shared with the gateway's token/subdomain index — keep it in sync
# with PORTEX_REDIS_URL.
REDIS_AUTH_INDEX_URL = f'{REDIS_URL}/0'

# Celery conf
CELERY_BROKER_URL = f'{REDIS_URL}/0'
CELERY_BROKER_TRANSPORT_OPTIONS = {'visibility_timeout': 3600}
CELERY_RESULT_BACKEND = f'{REDIS_URL}/0'
CELERY_ACCEPT_CONTENT = ['application/json']
CELERY_TASK_SERIALIZER = 'json'
CELERY_RESULT_SERIALIZER = 'json'


CACHES = {
    "default": {
        "BACKEND": "django.core.cache.backends.redis.RedisCache",
        # db 1 — keeps cache churn out of the auth index and the Celery queue.
        "LOCATION": f"{REDIS_URL}/1",
    }
}

DATA_UPLOAD_MAX_NUMBER_FIELDS = 10000

# Production hardening. Cookies go secure-only once DEBUG is off, which is
# safe because the gateway serves the apex over TLS and redirects port 80.
SECURE_COOKIES = not DEBUG
SESSION_COOKIE_SECURE = SECURE_COOKIES
CSRF_COOKIE_SECURE = SECURE_COOKIES
SESSION_COOKIE_HTTPONLY = True
SECURE_HSTS_SECONDS = 0 if DEBUG else int(os.getenv('SECURE_HSTS_SECONDS', 31536000))
# SECURE_HSTS_INCLUDE_SUBDOMAINS and SECURE_HSTS_PRELOAD stay off deliberately:
# includeSubDomains would force HTTPS on every user's tunnel subdomain, and
# preload is close to irreversible. Turn them on once that is a policy call
# you want to make, not because `check --deploy` mentions them.
SECURE_HSTS_INCLUDE_SUBDOMAINS = os.getenv('SECURE_HSTS_INCLUDE_SUBDOMAINS') == 'True'
SECURE_HSTS_PRELOAD = os.getenv('SECURE_HSTS_PRELOAD') == 'True'
SECURE_REFERRER_POLICY = 'same-origin'

# SECURE_SSL_REDIRECT stays off on purpose. The gateway splices raw bytes, so
# Django always sees a plain-HTTP request and would redirect forever. The
# gateway's port-80 listener issues the redirect instead.
SILENCED_SYSTEM_CHECKS = ['security.W008']

# Per-user quotas. Nothing here charges money yet, so the only thing stopping
# one account from claiming every good subdomain is these numbers. 0 disables
# a limit.
PORTEX_MAX_TOKENS_PER_USER = int(os.getenv('PORTEX_MAX_TOKENS_PER_USER', 10))
PORTEX_MAX_SUBDOMAINS_PER_USER = int(os.getenv('PORTEX_MAX_SUBDOMAINS_PER_USER', 3))

# Sign-in throttle, counted per client IP over a rolling window.
PORTEX_LOGIN_MAX_ATTEMPTS = int(os.getenv('PORTEX_LOGIN_MAX_ATTEMPTS', 10))
PORTEX_LOGIN_THROTTLE_SECONDS = int(os.getenv('PORTEX_LOGIN_THROTTLE_SECONDS', 300))

# Self-service dashboard auth
LOGIN_URL = '/accounts/login/'
LOGIN_REDIRECT_URL = '/dashboard/'
LOGOUT_REDIRECT_URL = '/'
