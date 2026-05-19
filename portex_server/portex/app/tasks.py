import requests

from app.models import DownloadApp
from celery import shared_task


@shared_task()
def save_downloader(meta):
    try:
        x_forwarded_for = meta.get('HTTP_X_FORWARDED_FOR')
        if x_forwarded_for:
            ip = x_forwarded_for.split(',')[0]
            print(ip)
        else:
            ip = meta.get('REMOTE_ADDR')
            print(ip, '----<>----')
        response = requests.get(f'https://ipinfo.io/{ip}/json')
        data = response.json()
        print(data)
        DownloadApp.objects.create(info=data)
    except Exception as data:
        DownloadApp.objects.create(info=data)

